//! Daemon-only subsystems used by the `parakit` binary.

/// Microphone capture and shared recording buffer.
#[path = "audio_manager.rs"]
pub(crate) mod audio;
/// Push-to-talk hotkey backends.
pub(crate) mod hotkey;
/// Text insertion at the focused cursor.
pub(crate) mod inject;
/// Terminal-aware daemon logging.
pub(crate) mod logging;
/// Runtime checks for desktop input permissions.
pub(crate) mod preflight;
/// Generated start/success/error sound cues.
pub(crate) mod sounds;
/// Shared X11 helpers for Linux desktop hotkeys and insertion checks.
#[cfg(target_os = "linux")]
pub(crate) mod x11;
