//! Recording coordinator between hotkey backends and the worker thread.

use super::{audio::AudioHandle, inject::FocusSnapshot, worker::WorkerEvent};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const MAX_UTTERANCE: Duration = Duration::from_secs(270);

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
    tx: Sender<WorkerEvent>,
    audio: AudioHandle,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("parakit-recording".into())
        .spawn(move || recording_coordinator_loop(rx, tx, audio))
}

fn recording_coordinator_loop(
    rx: Receiver<HotkeyTransition>,
    tx: Sender<WorkerEvent>,
    audio: AudioHandle,
) {
    recording_coordinator_loop_with_max_utterance(rx, tx, audio, MAX_UTTERANCE);
}

fn recording_coordinator_loop_with_max_utterance(
    rx: Receiver<HotkeyTransition>,
    tx: Sender<WorkerEvent>,
    audio: AudioHandle,
    max_utterance: Duration,
) {
    let mut started_at = None;
    let mut focus_at_start = None;

    while let Some(event) = next_coordinator_event(&rx, started_at, max_utterance) {
        match event {
            CoordinatorEvent::Hotkey(HotkeyTransition::Pressed { at }) if started_at.is_none() => {
                focus_at_start = FocusSnapshot::capture().ok();
                if let Err(err) = audio.start_recording() {
                    eprintln!("parakit: error: could not start audio recording: {err:#}");
                    focus_at_start = None;
                    continue;
                }
                match send_worker_event(&tx, WorkerEvent::RecordingStarted) {
                    WorkerSendStatus::Sent => {}
                    WorkerSendStatus::Full => {
                        let _ = audio.stop_recording();
                        focus_at_start = None;
                        continue;
                    }
                    WorkerSendStatus::Disconnected => break,
                }
                started_at = Some(at);
            }
            CoordinatorEvent::Hotkey(HotkeyTransition::Pressed { .. }) => {}
            CoordinatorEvent::Hotkey(HotkeyTransition::Released { at }) => {
                let Some(start) = started_at.take() else {
                    continue;
                };
                if stop_and_send_recording(&tx, &audio, start, at, &mut focus_at_start)
                    == WorkerSendStatus::Disconnected
                {
                    break;
                }
            }
            CoordinatorEvent::MaxUtterance => {
                let Some(start) = started_at.take() else {
                    continue;
                };
                if stop_and_send_recording(&tx, &audio, start, Instant::now(), &mut focus_at_start)
                    == WorkerSendStatus::Disconnected
                {
                    break;
                }
            }
        }
    }
}

enum CoordinatorEvent {
    Hotkey(HotkeyTransition),
    MaxUtterance,
}

fn next_coordinator_event(
    rx: &Receiver<HotkeyTransition>,
    started_at: Option<Instant>,
    max_utterance: Duration,
) -> Option<CoordinatorEvent> {
    let Some(start) = started_at else {
        return rx.recv().ok().map(CoordinatorEvent::Hotkey);
    };

    let elapsed = Instant::now().saturating_duration_since(start);
    let remaining = max_utterance.checked_sub(elapsed).unwrap_or(Duration::ZERO);
    match rx.recv_timeout(remaining) {
        Ok(transition) => Some(CoordinatorEvent::Hotkey(transition)),
        Err(RecvTimeoutError::Timeout) => Some(CoordinatorEvent::MaxUtterance),
        Err(RecvTimeoutError::Disconnected) => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WorkerSendStatus {
    Sent,
    Full,
    Disconnected,
}

fn stop_and_send_recording(
    tx: &Sender<WorkerEvent>,
    audio: &AudioHandle,
    started_at: Instant,
    stopped_at: Instant,
    focus_at_start: &mut Option<FocusSnapshot>,
) -> WorkerSendStatus {
    let pcm = match audio.stop_recording() {
        Ok(pcm) => pcm,
        Err(err) => {
            eprintln!("parakit: error: could not stop audio recording: {err:#}");
            focus_at_start.take();
            return WorkerSendStatus::Sent;
        }
    };
    send_worker_event(
        tx,
        WorkerEvent::RecordingStopped {
            started_at,
            stopped_at,
            pcm,
            focus_at_start: focus_at_start.take().map(Box::new),
        },
    )
}

fn send_worker_event(tx: &Sender<WorkerEvent>, event: WorkerEvent) -> WorkerSendStatus {
    match tx.try_send(event) {
        Ok(()) => WorkerSendStatus::Sent,
        Err(TrySendError::Full(_)) => {
            eprintln!("parakit: warning: transcription worker is busy; dropping recording");
            WorkerSendStatus::Full
        }
        Err(TrySendError::Disconnected(_)) => {
            eprintln!("parakit: error: transcription worker disconnected");
            WorkerSendStatus::Disconnected
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::{bounded, unbounded};

    #[test]
    fn coordinator_force_stops_at_max_utterance() {
        let audio = AudioHandle::test_handle();
        let (hotkey_tx, hotkey_rx) = unbounded();
        let (worker_tx, worker_rx) = bounded(2);
        let coordinator = thread::spawn(move || {
            recording_coordinator_loop_with_max_utterance(
                hotkey_rx,
                worker_tx,
                audio,
                Duration::from_millis(10),
            );
        });

        let started_at = Instant::now();
        hotkey_tx
            .send(HotkeyTransition::Pressed { at: started_at })
            .expect("hotkey press should send");

        assert!(matches!(
            worker_rx
                .recv_timeout(Duration::from_millis(250))
                .expect("start event"),
            WorkerEvent::RecordingStarted
        ));

        match worker_rx
            .recv_timeout(Duration::from_millis(500))
            .expect("timeout stop event")
        {
            WorkerEvent::RecordingStopped {
                started_at: start,
                stopped_at,
                pcm,
                ..
            } => {
                assert_eq!(start, started_at);
                assert!(stopped_at >= started_at);
                assert!(pcm.is_empty());
            }
            WorkerEvent::RecordingStarted => panic!("unexpected second start event"),
        }

        drop(hotkey_tx);
        coordinator.join().expect("coordinator should exit cleanly");
    }

    #[test]
    fn coordinator_drops_new_recording_when_worker_queue_is_full() {
        let audio = AudioHandle::test_handle();
        let (hotkey_tx, hotkey_rx) = unbounded();
        let (worker_tx, worker_rx) = bounded(0);
        let coordinator = thread::spawn(move || {
            recording_coordinator_loop_with_max_utterance(
                hotkey_rx,
                worker_tx,
                audio,
                Duration::from_millis(10),
            );
        });

        let started_at = Instant::now();
        hotkey_tx
            .send(HotkeyTransition::Pressed { at: started_at })
            .expect("hotkey press should send");
        hotkey_tx
            .send(HotkeyTransition::Released {
                at: started_at + Duration::from_millis(1),
            })
            .expect("hotkey release should send");
        drop(hotkey_tx);

        coordinator.join().expect("coordinator should exit cleanly");
        assert!(worker_rx.try_recv().is_err());
    }
}
