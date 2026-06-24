//! Daemon-only subsystems used by the `parakit` binary.

/// Captures microphone audio and reports input-device metadata.
pub(crate) mod audio;
/// Integrates global hotkeys, focus detection, and text insertion.
pub(crate) mod desktop;
/// Shared user-facing hotkey diagnostics.
pub(crate) mod hotkey_help;
/// Coordinates single-instance daemon IPC.
pub(crate) mod ipc;
/// Writes transcript logs and runtime event records.
pub(crate) mod logging;
#[cfg(target_os = "macos")]
/// macOS desktop permission, focus, and diagnostic helpers.
pub(crate) mod macos;
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
/// Runs transcription and post-processing work off the hotkey thread.
pub(crate) mod worker;

/// Re-export commonly used desktop helpers for the daemon entrypoint.
pub(crate) use desktop::{hotkey, inject};

#[cfg(target_os = "linux")]
/// Re-export Linux desktop/session helpers.
pub(crate) use desktop::{session, wsl, x11};

#[cfg(target_os = "windows")]
#[allow(unused_imports)]
/// Re-export Windows desktop integration helpers.
pub(crate) use desktop::{windows_focus, windows_input, windows_paste_smoke, windows_security};
