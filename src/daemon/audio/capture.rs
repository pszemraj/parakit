//! Microphone capture into a shared f32 buffer at 16 kHz mono.
//!
//! The CPAL stream is owned by a dedicated manager thread. That keeps stream
//! creation and teardown on one thread, lets the daemon reopen the stream when
//! the OS default input changes, and avoids crashing when a USB or Bluetooth
//! microphone disappears.

use anyhow::{anyhow, Context, Result};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TrySendError};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::daemon::logging::Logger;
use crate::daemon::notifications::Notifier;
use crate::daemon::recording::MAX_UTTERANCE_SECONDS;

pub use parakit::constants::TARGET_RATE;

#[path = "capture_device.rs"]
mod device;
use device::selected_mic_info;
pub use device::MicInfo;
#[path = "capture_drain.rs"]
mod drain;
#[path = "capture_stream.rs"]
mod stream;
use stream::audio_manager_loop;

/// Reusable capacity for ordinary dictation bursts.
const RECORDING_CAPACITY: usize = TARGET_RATE as usize * 90;
/// Hard cap for one held recording to prevent unbounded memory growth.
const RECORDING_HARD_CAP_HEADROOM_SECONDS: usize = 30;
const MAX_RECORDING_SAMPLES: usize =
    TARGET_RATE as usize * (MAX_UTTERANCE_SECONDS as usize + RECORDING_HARD_CAP_HEADROOM_SECONDS);
const PRE_ROLL_SAMPLES: usize = TARGET_RATE as usize * 350 / 1000;
const AUDIO_CONTROL_TIMEOUT: Duration = Duration::from_secs(1);

/// Send-Sync handle that worker threads use to control and read the buffer.
#[derive(Clone)]
pub struct AudioHandle {
    state: Arc<Mutex<CaptureState>>,
    session_epoch: Arc<AtomicU64>,
    next_session_epoch: Arc<AtomicU64>,
    control: Sender<AudioControl>,
}

impl AudioHandle {
    /// Clear the current buffer, seed it with pre-roll, and begin recording.
    ///
    /// # Returns
    ///
    /// `Ok(())` when the live audio manager acknowledges the recording start.
    ///
    /// # Errors
    ///
    /// Returns an error if the audio manager is unavailable, cannot accept the
    /// command, or does not acknowledge it before the control timeout.
    pub fn start_recording(&self) -> Result<()> {
        let next = self
            .next_session_epoch
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
            .max(1);

        self.start_on_manager(next)
    }

    /// Stop recording and take ownership of the buffered samples.
    ///
    /// # Returns
    ///
    /// The captured mono PCM samples at [`TARGET_RATE`].
    ///
    /// # Errors
    ///
    /// Returns an error if the audio manager is unavailable, cannot accept the
    /// command, or does not acknowledge it before the control timeout.
    /// Recording state is reset locally before the error is returned.
    pub fn stop_recording(&self) -> Result<Vec<f32>> {
        match self.stop_on_manager() {
            Ok(pcm) => Ok(pcm),
            Err(err) => {
                self.reset_recording_after_failed_stop();
                Err(err)
            }
        }
    }

    fn reset_recording_after_failed_stop(&self) {
        self.session_epoch.store(0, Ordering::Release);
        let _ = self.state.lock().take_recording();
    }

    fn start_on_manager(&self, epoch: u64) -> Result<()> {
        let (ack_tx, ack_rx) = bounded(1);
        send_audio_control(&self.control, AudioControl::Start { epoch, ack: ack_tx })?;
        recv_audio_control_ack(ack_rx, "audio manager", "Start")
    }

    fn stop_on_manager(&self) -> Result<Vec<f32>> {
        let (ack_tx, ack_rx) = bounded(1);
        send_audio_control(&self.control, AudioControl::Stop { ack: ack_tx })?;
        recv_audio_control_ack(ack_rx, "audio manager", "Stop")
    }
}

fn send_audio_control(control: &Sender<AudioControl>, command: AudioControl) -> Result<()> {
    match control.try_send(command) {
        Ok(()) => Ok(()),
        Err(TrySendError::Disconnected(_)) => Err(anyhow!(
            "audio manager is not running; recording command was not accepted"
        )),
        Err(TrySendError::Full(_)) => Err(anyhow!(
            "audio manager control queue is full; recording command was not accepted"
        )),
    }
}

fn recv_audio_control_ack<T>(
    ack_rx: Receiver<Result<T>>,
    layer: &'static str,
    label: &'static str,
) -> Result<T> {
    match ack_rx.recv_timeout(AUDIO_CONTROL_TIMEOUT) {
        Ok(result) => result,
        Err(err) => Err(audio_control_ack_error(layer, label, err)),
    }
}

fn audio_control_ack_error(
    layer: &'static str,
    label: &'static str,
    err: RecvTimeoutError,
) -> anyhow::Error {
    match err {
        RecvTimeoutError::Timeout => {
            anyhow!("{layer} accepted {label} but did not acknowledge before timeout")
        }
        RecvTimeoutError::Disconnected => {
            anyhow!("{layer} accepted {label} but disconnected before acknowledging")
        }
    }
}

#[cfg(test)]
impl AudioHandle {
    /// Build an isolated audio handle for coordinator and buffer unit tests.
    ///
    /// # Returns
    ///
    /// A handle with an empty buffer, closed epoch, and an acknowledging test
    /// manager.
    ///
    /// # Panics
    ///
    /// Panics if the test audio-manager thread cannot be spawned.
    pub(crate) fn test_handle() -> Self {
        let state = Arc::new(Mutex::new(CaptureState::new()));
        let session_epoch = Arc::new(AtomicU64::new(0));
        let (control, control_rx) = bounded(4);
        let manager_state = Arc::clone(&state);
        let manager_epoch = Arc::clone(&session_epoch);
        thread::Builder::new()
            .name("parakit-test-audio".into())
            .spawn(move || {
                while let Ok(command) = control_rx.recv() {
                    match command {
                        AudioControl::Start { epoch, ack } => {
                            manager_state.lock().begin_recording();
                            manager_epoch.store(epoch, Ordering::Release);
                            let _ = ack.send(Ok(()));
                        }
                        AudioControl::Stop { ack } => {
                            manager_epoch.store(0, Ordering::Release);
                            let pcm = manager_state.lock().take_recording();
                            let _ = ack.send(Ok(pcm));
                        }
                    }
                }
            })
            .expect("spawn test audio manager");
        Self {
            state,
            session_epoch,
            next_session_epoch: Arc::new(AtomicU64::new(0)),
            control,
        }
    }

    /// Return whether the test handle currently considers recording active.
    ///
    /// # Returns
    ///
    /// `true` when the session epoch is non-zero.
    pub(crate) fn test_is_recording(&self) -> bool {
        self.session_epoch.load(Ordering::Acquire) != 0
    }
}

/// Live audio capture manager.
pub struct AudioCapture {
    /// Cloneable buffer control handle used by worker and hotkey threads.
    pub handle: AudioHandle,
    current: Arc<Mutex<Option<MicInfo>>>,
    alive: Arc<AtomicBool>,
    _thread: JoinHandle<()>,
}

#[derive(Default)]
struct CaptureState {
    buffer: Vec<f32>,
    pre_roll: VecDeque<f32>,
}

impl CaptureState {
    fn new() -> Self {
        Self {
            buffer: Vec::with_capacity(RECORDING_CAPACITY),
            pre_roll: VecDeque::with_capacity(PRE_ROLL_SAMPLES),
        }
    }

    fn begin_recording(&mut self) {
        self.begin_recording_with_pre_roll(true);
    }

    fn begin_recording_without_pre_roll(&mut self) {
        self.begin_recording_with_pre_roll(false);
    }

    fn begin_recording_with_pre_roll(&mut self, include_pre_roll: bool) {
        if self.buffer.capacity() < RECORDING_CAPACITY {
            self.buffer
                .reserve_exact(RECORDING_CAPACITY - self.buffer.capacity());
        }
        self.buffer.clear();
        if include_pre_roll {
            self.buffer.extend(self.pre_roll.iter().copied());
        }
        self.pre_roll.clear();
    }

    fn push_pre_roll(&mut self, samples: &[f32]) {
        if samples.len() >= PRE_ROLL_SAMPLES {
            self.pre_roll.clear();
            self.pre_roll
                .extend(samples[samples.len() - PRE_ROLL_SAMPLES..].iter().copied());
            return;
        }
        let excess = self.pre_roll.len() + samples.len();
        if excess > PRE_ROLL_SAMPLES {
            self.pre_roll.drain(..excess - PRE_ROLL_SAMPLES);
        }
        self.pre_roll.extend(samples.iter().copied());
    }

    fn append_recording(&mut self, samples: &[f32]) {
        append_samples_bounded(&mut self.buffer, samples);
    }

    fn take_recording(&mut self) -> Vec<f32> {
        std::mem::replace(&mut self.buffer, Vec::with_capacity(RECORDING_CAPACITY))
    }
}

impl AudioCapture {
    /// Open the best available input device and start the manager thread.
    ///
    /// # Returns
    ///
    /// A live capture manager plus a cloneable [`AudioHandle`].
    ///
    /// # Arguments
    ///
    /// * `log` - Logger used for device-change and recovery messages.
    /// * `notifier` - Desktop notification helper for microphone failures.
    ///
    /// # Errors
    ///
    /// Returns an error if no usable input device can be opened.
    pub fn open(log: Arc<Logger>, notifier: Notifier) -> Result<Self> {
        #[cfg(target_os = "macos")]
        crate::daemon::macos::microphone_preflight()?;

        let state = Arc::new(Mutex::new(CaptureState::new()));
        let session_epoch = Arc::new(AtomicU64::new(0));
        let current = Arc::new(Mutex::new(None));
        let alive = Arc::new(AtomicBool::new(true));
        let stream_error = Arc::new(Mutex::new(None));
        let (control_tx, control_rx) = bounded::<AudioControl>(4);

        let handle = AudioHandle {
            state: Arc::clone(&state),
            session_epoch: Arc::clone(&session_epoch),
            next_session_epoch: Arc::new(AtomicU64::new(0)),
            control: control_tx,
        };

        let (ready_tx, ready_rx) = bounded::<Result<MicInfo>>(1);
        let thread_current = Arc::clone(&current);
        let thread_alive = Arc::clone(&alive);
        let thread_error = Arc::clone(&stream_error);
        let thread_log = Arc::clone(&log);

        let manager = thread::Builder::new()
            .name("parakit-audio".into())
            .spawn(move || {
                audio_manager_loop(AudioManagerCtx {
                    state,
                    session_epoch,
                    current: thread_current,
                    alive: thread_alive,
                    stream_error: thread_error,
                    control_rx,
                    log: thread_log,
                    notifier,
                    ready: ready_tx,
                });
            })
            .context("spawn audio manager")?;

        ready_rx
            .recv()
            .context("audio manager stopped before reporting startup")??;

        Ok(Self {
            handle,
            current,
            alive,
            _thread: manager,
        })
    }

    /// Return the current microphone summary.
    ///
    /// # Returns
    ///
    /// The last successfully opened microphone stream, if any.
    pub fn mic_info(&self) -> Option<MicInfo> {
        self.current.lock().clone()
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
    }
}

struct AudioManagerCtx {
    state: Arc<Mutex<CaptureState>>,
    session_epoch: Arc<AtomicU64>,
    current: Arc<Mutex<Option<MicInfo>>>,
    alive: Arc<AtomicBool>,
    stream_error: Arc<Mutex<Option<String>>>,
    control_rx: Receiver<AudioControl>,
    log: Arc<Logger>,
    notifier: Notifier,
    ready: crossbeam_channel::Sender<Result<MicInfo>>,
}

enum AudioControl {
    Start { epoch: u64, ack: Sender<Result<()>> },
    Stop { ack: Sender<Result<Vec<f32>>> },
}

/// Probe the currently selected input without opening a stream.
///
/// # Returns
///
/// The microphone parakit would currently try to use.
///
/// # Errors
///
/// Returns an error if no usable input device is available.
pub fn probe_default_input() -> Result<MicInfo> {
    #[cfg(target_os = "macos")]
    crate::daemon::macos::microphone_preflight()?;

    let host = cpal::default_host();
    selected_mic_info(&host)
}

fn append_processed_samples(
    state: &Mutex<CaptureState>,
    session_epoch: &AtomicU64,
    samples: &[f32],
) {
    let observed_epoch = session_epoch.load(Ordering::Acquire);
    append_processed_samples_observed(state, session_epoch, observed_epoch, samples);
}

fn append_processed_samples_observed(
    state: &Mutex<CaptureState>,
    session_epoch: &AtomicU64,
    observed_epoch: u64,
    samples: &[f32],
) {
    let mut state = state.lock();
    let current_epoch = session_epoch.load(Ordering::Acquire);

    if observed_epoch == 0 && current_epoch == 0 {
        state.push_pre_roll(samples);
    } else if observed_epoch != 0 && current_epoch == observed_epoch {
        state.append_recording(samples);
    }
}

fn append_samples_bounded(buf: &mut Vec<f32>, samples: &[f32]) {
    if buf.len() >= MAX_RECORDING_SAMPLES {
        return;
    }

    let remaining = MAX_RECORDING_SAMPLES - buf.len();
    let n = samples.len().min(remaining);
    buf.extend_from_slice(&samples[..n]);
}

#[cfg(test)]
#[path = "capture_tests.rs"]
mod capture_tests;
