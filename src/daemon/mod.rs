//! Daemon-only subsystems used by the `parakit` binary.

/// Linux libasound stderr handling.
#[cfg(target_os = "linux")]
pub(crate) mod alsa;
/// Microphone capture and shared recording buffer.
#[path = "audio_manager.rs"]
pub(crate) mod audio;
/// Linux PulseAudio/PipeWire source metadata parsing.
#[cfg(target_os = "linux")]
pub(crate) mod audio_pactl;
/// Push-to-talk hotkey backends.
pub(crate) mod hotkey;
/// Text insertion at the focused cursor.
pub(crate) mod inject;
/// Local daemon control socket.
pub(crate) mod ipc;
/// Terminal-aware daemon logging.
pub(crate) mod logging;
/// Desktop notifications for copy-only and device fallbacks.
pub(crate) mod notifications;
/// Runtime checks for desktop input permissions.
pub(crate) mod preflight;
/// Coordinator between hotkey transitions and audio recording events.
pub(crate) mod recording;
/// Desktop session compatibility checks.
#[cfg(target_os = "linux")]
pub(crate) mod session;
/// Generated start/success/error sound cues.
pub(crate) mod sounds;
/// Paste-target safety inspection.
pub(crate) mod target;
/// Transcription worker, sanitizer, and paste safety flow.
pub(crate) mod worker;
/// Shared X11 helpers for Linux desktop hotkeys and insertion checks.
#[cfg(target_os = "linux")]
pub(crate) mod x11;
