//! Audio capture, Linux ALSA handling, and source metadata helpers.

#[cfg(target_os = "linux")]
/// Linux ALSA device discovery and metadata helpers.
pub(crate) mod alsa;

mod capture;

#[cfg(target_os = "linux")]
mod pactl;

/// Re-export audio capture primitives used by the daemon.
pub(crate) use capture::{probe_default_input, AudioCapture, AudioHandle, MicInfo, TARGET_RATE};
