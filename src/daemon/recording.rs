//! Recording coordinator between hotkey backends and the worker thread.

use super::{audio::AudioHandle, inject::FocusSnapshot, logging::Logger, worker::WorkerEvent};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Hard stop for one held push-to-talk capture.
pub(crate) const MAX_UTTERANCE_SECONDS: u64 = 270;
const MAX_UTTERANCE: Duration = Duration::from_secs(MAX_UTTERANCE_SECONDS);

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
/// * `log` - Shared daemon logger used for warnings and errors raised on this thread.
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
    log: Arc<Logger>,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("parakit-recording".into())
        .spawn(move || recording_coordinator_loop(rx, tx, audio, log.as_ref()))
}

fn recording_coordinator_loop(
    rx: Receiver<HotkeyTransition>,
    tx: Sender<WorkerEvent>,
    audio: AudioHandle,
    log: &Logger,
) {
    recording_coordinator_loop_with_max_utterance(rx, tx, audio, MAX_UTTERANCE, log);
}

fn recording_coordinator_loop_with_max_utterance(
    rx: Receiver<HotkeyTransition>,
    tx: Sender<WorkerEvent>,
    audio: AudioHandle,
    max_utterance: Duration,
    log: &Logger,
) {
    let mut started_at = None;
    let mut focus_at_start = None;

    while let Some(event) = next_coordinator_event(&rx, started_at, max_utterance) {
        let stopped_at = match event {
            CoordinatorEvent::Hotkey(HotkeyTransition::Released { at }) => Some(at),
            CoordinatorEvent::MaxUtterance => Some(Instant::now()),
            CoordinatorEvent::Hotkey(HotkeyTransition::Pressed { .. }) => None,
        };
        if let Some(stopped_at) = stopped_at {
            if let Some(started_at) = started_at.take() {
                if stop_and_send_recording(
                    &tx,
                    &audio,
                    started_at,
                    stopped_at,
                    &mut focus_at_start,
                    log,
                ) == WorkerSendStatus::Disconnected
                {
                    break;
                }
            }
            continue;
        }

        match event {
            CoordinatorEvent::Hotkey(HotkeyTransition::Pressed { at }) if started_at.is_none() => {
                focus_at_start = FocusSnapshot::capture().ok();
                if let Err(err) = audio.start_recording() {
                    log.error(&format!("could not start audio recording: {err:#}"));
                    focus_at_start = None;
                    continue;
                }
                match try_send_worker_event(&tx, WorkerEvent::Started, log) {
                    WorkerSendStatus::Sent => {}
                    WorkerSendStatus::Full => {
                        stop_rejected_recording(
                            &audio,
                            "worker queue rejected recording start",
                            log,
                        );
                        focus_at_start = None;
                        continue;
                    }
                    WorkerSendStatus::Disconnected => {
                        stop_rejected_recording(
                            &audio,
                            "worker disconnected after recording start",
                            log,
                        );
                        break;
                    }
                }
                started_at = Some(at);
            }
            CoordinatorEvent::Hotkey(HotkeyTransition::Pressed { .. }) => {}
            CoordinatorEvent::Hotkey(HotkeyTransition::Released { .. })
            | CoordinatorEvent::MaxUtterance => unreachable!("handled above"),
        }
    }
}

#[derive(Clone, Copy)]
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
    log: &Logger,
) -> WorkerSendStatus {
    send_recording_result(
        tx,
        audio.stop_recording(),
        started_at,
        stopped_at,
        focus_at_start,
        log,
    )
}

fn send_recording_result(
    tx: &Sender<WorkerEvent>,
    recording: anyhow::Result<Vec<f32>>,
    started_at: Instant,
    stopped_at: Instant,
    focus_at_start: &mut Option<FocusSnapshot>,
    log: &Logger,
) -> WorkerSendStatus {
    let pcm = match recording {
        Ok(pcm) => pcm,
        Err(err) => {
            let message = format!("could not stop audio recording: {err:#}");
            focus_at_start.take();
            return send_required_worker_event(tx, WorkerEvent::Failed { message }, log);
        }
    };
    send_required_worker_event(
        tx,
        WorkerEvent::Stopped {
            started_at,
            stopped_at,
            pcm,
            focus_at_start: focus_at_start.take().map(Box::new),
        },
        log,
    )
}

fn stop_rejected_recording(audio: &AudioHandle, context: &'static str, log: &Logger) {
    if let Err(err) = audio.stop_recording() {
        log.warn(format!("could not stop recording after {context}: {err:#}"));
    }
}

fn try_send_worker_event(
    tx: &Sender<WorkerEvent>,
    event: WorkerEvent,
    log: &Logger,
) -> WorkerSendStatus {
    match tx.try_send(event) {
        Ok(()) => WorkerSendStatus::Sent,
        Err(TrySendError::Full(_)) => {
            log.warn("transcription worker is busy; dropping recording");
            WorkerSendStatus::Full
        }
        Err(TrySendError::Disconnected(_)) => {
            log.error("transcription worker disconnected");
            WorkerSendStatus::Disconnected
        }
    }
}

fn send_required_worker_event(
    tx: &Sender<WorkerEvent>,
    event: WorkerEvent,
    log: &Logger,
) -> WorkerSendStatus {
    match tx.send(event) {
        Ok(()) => WorkerSendStatus::Sent,
        Err(_) => {
            log.error("transcription worker disconnected");
            WorkerSendStatus::Disconnected
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::logging::LogLevel;
    use super::*;
    use crossbeam_channel::{bounded, unbounded};

    const EVENT_TIMEOUT: Duration = Duration::from_secs(2);

    fn assert_empty_stopped_event(event: WorkerEvent, started_at: Instant) {
        match event {
            WorkerEvent::Stopped {
                started_at: start,
                stopped_at,
                pcm,
                ..
            } => {
                assert_eq!(start, started_at);
                assert!(stopped_at >= started_at);
                assert!(pcm.is_empty());
            }
            WorkerEvent::Started => panic!("unexpected second start event"),
            WorkerEvent::Failed { message } => {
                panic!("unexpected recording failure event: {message}")
            }
        }
    }

    #[test]
    fn coordinator_force_stops_at_max_utterance() {
        let audio = AudioHandle::test_handle();
        let log = Logger::new(LogLevel::Quiet);
        let (hotkey_tx, hotkey_rx) = unbounded();
        let (worker_tx, worker_rx) = bounded(2);
        let coordinator = thread::spawn(move || {
            recording_coordinator_loop_with_max_utterance(
                hotkey_rx,
                worker_tx,
                audio,
                Duration::from_millis(10),
                &log,
            );
        });

        let started_at = Instant::now();
        hotkey_tx
            .send(HotkeyTransition::Pressed { at: started_at })
            .expect("hotkey press should send");

        assert!(matches!(
            worker_rx.recv_timeout(EVENT_TIMEOUT).expect("start event"),
            WorkerEvent::Started
        ));

        assert_empty_stopped_event(
            worker_rx
                .recv_timeout(EVENT_TIMEOUT)
                .expect("timeout stop event"),
            started_at,
        );

        drop(hotkey_tx);
        coordinator.join().expect("coordinator should exit cleanly");
    }

    #[test]
    fn terminal_failure_is_delivered_after_started_when_worker_queue_is_full() {
        let (worker_tx, worker_rx) = bounded(1);
        worker_tx
            .try_send(WorkerEvent::Started)
            .expect("preload started event");
        let started_at = Instant::now();
        let stopped_at = started_at + Duration::from_millis(5);
        let log = Logger::new(LogLevel::Quiet);
        let sender = thread::spawn(move || {
            let mut focus_at_start = None;
            send_recording_result(
                &worker_tx,
                Err(anyhow::anyhow!(
                    "audio drain accepted Stop but did not acknowledge before timeout"
                )),
                started_at,
                stopped_at,
                &mut focus_at_start,
                &log,
            )
        });

        assert!(matches!(
            worker_rx.recv_timeout(EVENT_TIMEOUT).expect("start event"),
            WorkerEvent::Started
        ));
        match worker_rx
            .recv_timeout(EVENT_TIMEOUT)
            .expect("terminal failure event")
        {
            WorkerEvent::Failed { message } => {
                assert!(message.contains("could not stop audio recording"));
                assert!(message.contains("accepted Stop"));
            }
            WorkerEvent::Started => panic!("unexpected second start event"),
            WorkerEvent::Stopped { .. } => panic!("unexpected stop event"),
        }
        assert_eq!(
            sender.join().expect("sender thread should finish"),
            WorkerSendStatus::Sent
        );
    }

    #[test]
    fn coordinator_drops_new_recording_when_worker_queue_is_full() {
        let audio = AudioHandle::test_handle();
        let observed_audio = audio.clone();
        let log = Logger::new(LogLevel::Quiet);
        let (hotkey_tx, hotkey_rx) = unbounded();
        let (worker_tx, worker_rx) = bounded(0);
        let coordinator = thread::spawn(move || {
            recording_coordinator_loop_with_max_utterance(
                hotkey_rx,
                worker_tx,
                audio,
                Duration::from_millis(10),
                &log,
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
        assert!(!observed_audio.test_is_recording());
    }

    #[test]
    fn coordinator_stops_recording_when_worker_disconnects_after_start() {
        let audio = AudioHandle::test_handle();
        let observed_audio = audio.clone();
        let log = Logger::new(LogLevel::Quiet);
        let (hotkey_tx, hotkey_rx) = unbounded();
        let (worker_tx, worker_rx) = bounded(1);
        drop(worker_rx);
        let coordinator = thread::spawn(move || {
            recording_coordinator_loop_with_max_utterance(
                hotkey_rx,
                worker_tx,
                audio,
                Duration::from_millis(10),
                &log,
            );
        });

        hotkey_tx
            .send(HotkeyTransition::Pressed { at: Instant::now() })
            .expect("hotkey press should send");
        drop(hotkey_tx);

        coordinator.join().expect("coordinator should exit cleanly");
        assert!(!observed_audio.test_is_recording());
    }

    #[test]
    fn coordinator_delivers_terminal_event_after_accepted_start_when_queue_is_full() {
        let audio = AudioHandle::test_handle();
        let log = Logger::new(LogLevel::Quiet);
        let (hotkey_tx, hotkey_rx) = unbounded();
        let (worker_tx, worker_rx) = bounded(1);
        let coordinator = thread::spawn(move || {
            recording_coordinator_loop_with_max_utterance(
                hotkey_rx,
                worker_tx,
                audio,
                Duration::from_millis(250),
                &log,
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

        assert!(matches!(
            worker_rx.recv_timeout(EVENT_TIMEOUT).expect("start event"),
            WorkerEvent::Started
        ));
        assert_empty_stopped_event(
            worker_rx
                .recv_timeout(EVENT_TIMEOUT)
                .expect("terminal stop event"),
            started_at,
        );

        drop(hotkey_tx);
        coordinator.join().expect("coordinator should exit cleanly");
    }
}
