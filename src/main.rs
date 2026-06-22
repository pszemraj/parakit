//! parakit - a push-to-talk dictation daemon.
//!
//! Architecture:
//!   - Main thread: parse CLI, set up subsystems, then run the hotkey backend.
//!     The hotkey loop is blocking and runs forever until SIGINT.
//!   - Recording coordinator thread: converts hotkey transitions into audio
//!     start/stop calls and owned PCM worker events.
//!   - Audio manager thread: owns the live cpal stream and follows the default
//!     input device.
//!   - cpal callback thread: mixes mic samples to mono and pushes them into a
//!     bounded SPSC ring for the audio drain thread.
//!   - Worker thread: receives Event messages via crossbeam-channel, runs
//!     transcription off the hotkey thread so input stays responsive.
//!
//! State machine (single-recording-at-a-time invariant):
//!   Idle --[Ctrl+Space down]--> Recording --[Ctrl+Space up]--> Transcribing --> Idle
//!
//! On Linux, `auto` registers Ctrl+Space with the X11 desktop. The evdev/uinput
//! keyboard proxy is explicit and experimental.

mod app;
mod cli;
mod daemon;
#[cfg(test)]
mod test_support;

fn main() {
    if let Err(err) = app::run() {
        eprintln!("parakit: error: {err:#}");
        std::process::exit(1);
    }
}
