//! Recording coordinator between hotkey backends and the worker thread.

use super::{audio::AudioHandle, inject::FocusSnapshot};
use crate::Event_;
use crossbeam_channel::{Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Instant;

/// Logical push-to-talk transition emitted by platform hotkey backends.
///
/// Backends should stay on the OS input boundary. The coordinator consumes
/// these transitions and owns focus capture, audio start/stop, and PCM handoff.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HotkeyTransition {
    /// The configured push-to-talk chord became active.
    Pressed {
        /// Monotonic timestamp captured at hotkey activation.
        at: Instant,
    },
    /// The configured push-to-talk chord became inactive.
    Released {
        /// Monotonic timestamp captured at hotkey release.
        at: Instant,
    },
}

/// Start the coordinator that converts hotkey transitions into worker events.
///
/// # Arguments
///
/// * `rx` - Logical push-to-talk transitions from the active hotkey backend.
/// * `tx` - Worker event channel used to post recording events.
/// * `audio` - Audio capture handle owned by this coordinator boundary.
///
/// # Returns
///
/// A join handle for the coordinator thread.
///
/// # Errors
///
/// Returns an error if the coordinator thread cannot be spawned.
pub(crate) fn spawn_recording_coordinator(
    rx: Receiver<HotkeyTransition>,
    tx: Sender<Event_>,
    audio: AudioHandle,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("parakit-recording".into())
        .spawn(move || recording_coordinator_loop(rx, tx, audio))
}

fn recording_coordinator_loop(
    rx: Receiver<HotkeyTransition>,
    tx: Sender<Event_>,
    audio: AudioHandle,
) {
    let mut started_at = None;

    while let Ok(transition) = rx.recv() {
        match transition {
            HotkeyTransition::Pressed { at } if started_at.is_none() => {
                let focus = FocusSnapshot::capture().ok();
                audio.start_recording();
                started_at = Some(at);
                let _ = tx.send(Event_::RecordingStarted { focus });
            }
            HotkeyTransition::Pressed { .. } => {}
            HotkeyTransition::Released { at } => {
                let Some(start) = started_at.take() else {
                    continue;
                };
                let pcm = audio.stop_recording();
                let _ = tx.send(Event_::RecordingStopped {
                    started_at: start,
                    stopped_at: at,
                    pcm,
                });
            }
        }
    }
}
