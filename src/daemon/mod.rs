//! Daemon-only subsystems used by the `parakit` binary.

/// Microphone capture and shared recording buffer.
pub(crate) mod audio;
/// Synthetic typing at the focused cursor.
pub(crate) mod inject;
/// Generated start/success/error sound cues.
pub(crate) mod sounds;
