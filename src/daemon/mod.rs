//! Daemon-only subsystems used by the `parakit` binary.

/// Microphone capture and shared recording buffer.
#[path = "audio_manager.rs"]
pub(crate) mod audio;
/// Synthetic typing at the focused cursor.
pub(crate) mod inject;
/// Terminal-aware daemon logging.
pub(crate) mod logging;
/// Runtime checks for desktop input permissions.
pub(crate) mod preflight;
/// Generated start/success/error sound cues.
pub(crate) mod sounds;
