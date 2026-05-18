//! Audio capture, Linux ALSA handling, and source metadata helpers.

#[cfg(target_os = "linux")]
pub(crate) mod alsa;

mod capture;

#[cfg(target_os = "linux")]
mod pactl;

pub(crate) use capture::{probe_default_input, AudioCapture, AudioHandle, MicInfo, TARGET_RATE};
