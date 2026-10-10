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
mod config;
mod daemon;
#[cfg(test)]
mod test_support;

use std::io::Write as _;

fn main() {
    if let Err(err) = app::run() {
        // A closed launching terminal must not turn exit status 1 into a panic.
        let _ = writeln!(std::io::stderr(), "parakit: error: {err:#}");
        std::process::exit(1);
    }
}
