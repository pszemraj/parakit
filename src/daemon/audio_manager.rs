//! Microphone capture into a shared f32 buffer at 16 kHz mono.
//!
//! The CPAL stream is owned by a dedicated manager thread. That keeps stream
//! creation and teardown on one thread, lets the daemon reopen the stream when
//! the OS default input changes, and avoids crashing when a USB or Bluetooth
//! microphone disappears.

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TrySendError};
use parakit::audio_file::{resampler_params, RESAMPLE_CHUNK_SIZE};
use parking_lot::Mutex;
use ringbuf::{
    traits::{Consumer, Producer, Split},
    HeapCons, HeapProd, HeapRb,
};
use rubato::{Resampler, SincFixedIn};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::{logging::Logger, notifications::Notifier};
#[cfg(target_os = "linux")]
use crate::daemon::audio_pactl::{pactl_default_source_info, pactl_default_source_name};

pub use parakit::constants::TARGET_RATE;

/// Reusable capacity for ordinary dictation bursts.
const RECORDING_CAPACITY: usize = TARGET_RATE as usize * 90;
/// Hard cap for one held recording to prevent unbounded memory growth.
const MAX_RECORDING_SAMPLES: usize = TARGET_RATE as usize * 60 * 5;
const PRE_ROLL_SAMPLES: usize = TARGET_RATE as usize * 350 / 1000;
const AUDIO_RING_SECONDS: usize = 6;
const AUDIO_RING_MIN_CAPACITY: usize = TARGET_RATE as usize * AUDIO_RING_SECONDS;
const DEFAULT_CALLBACK_SCRATCH_FRAMES: usize = 8192;
const AUDIO_CONTROL_TIMEOUT: Duration = Duration::from_secs(1);
const DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(1);
const DEVICE_RETRY_MAX_INTERVAL: Duration = Duration::from_secs(10);

/// Send-Sync handle that worker threads use to control and read the buffer.
#[derive(Clone)]
pub struct AudioHandle {
    state: Arc<Mutex<CaptureState>>,
    session_epoch: Arc<AtomicU64>,
    next_session_epoch: Arc<AtomicU64>,
    control: Arc<Mutex<Option<Sender<AudioControl>>>>,
}

impl AudioHandle {
    /// Clear the current buffer, seed it with pre-roll, and begin recording.
    ///
    /// # Returns
    ///
    /// `Ok(())` when recording state was started by the live drain thread or
    /// by the no-drain fallback path.
    ///
    /// # Errors
    ///
    /// Returns an error if the live audio drain accepts the command but does
    /// not acknowledge it before the control timeout.
    pub fn start_recording(&self) -> Result<()> {
        let next = self
            .next_session_epoch
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
            .max(1);

        match self.try_start_on_drain(next)? {
            AudioControlAck::Acked(()) => Ok(()),
            AudioControlAck::NoLiveDrain => {
                let mut state = self.state.lock();
                state.begin_recording();
                self.session_epoch.store(next, Ordering::Release);
                Ok(())
            }
        }
    }

    /// Stop recording and take ownership of the buffered samples.
    ///
    /// # Returns
    ///
    /// The captured mono PCM samples at [`TARGET_RATE`].
    ///
    /// # Errors
    ///
    /// Returns an error if the live audio drain accepts the command but does
    /// not acknowledge it before the control timeout.
    pub fn stop_recording(&self) -> Result<Vec<f32>> {
        match self.try_stop_on_drain()? {
            AudioControlAck::Acked(pcm) => Ok(pcm),
            AudioControlAck::NoLiveDrain => {
                self.session_epoch.store(0, Ordering::Release);
                Ok(self.state.lock().take_recording())
            }
        }
    }

    fn try_start_on_drain(&self, epoch: u64) -> Result<AudioControlAck<()>> {
        let Some(control) = self.control.lock().clone() else {
            return Ok(AudioControlAck::NoLiveDrain);
        };
        let (ack_tx, ack_rx) = bounded(1);
        if !try_send_audio_control(control, AudioControl::Start { epoch, ack: ack_tx })? {
            return Ok(AudioControlAck::NoLiveDrain);
        }
        recv_audio_control_ack(ack_rx, "Start").map(AudioControlAck::Acked)
    }

    fn try_stop_on_drain(&self) -> Result<AudioControlAck<Vec<f32>>> {
        let Some(control) = self.control.lock().clone() else {
            return Ok(AudioControlAck::NoLiveDrain);
        };
        let (ack_tx, ack_rx) = bounded(1);
        if !try_send_audio_control(control, AudioControl::Stop { ack: ack_tx })? {
            return Ok(AudioControlAck::NoLiveDrain);
        }
        recv_audio_control_ack(ack_rx, "Stop").map(AudioControlAck::Acked)
    }
}

enum AudioControlAck<T> {
    Acked(T),
    NoLiveDrain,
}

fn try_send_audio_control(control: Sender<AudioControl>, command: AudioControl) -> Result<bool> {
    match control.try_send(command) {
        Ok(()) => Ok(true),
        Err(TrySendError::Disconnected(_)) => Ok(false),
        Err(TrySendError::Full(_)) => Err(anyhow!(
            "audio manager control queue is full; recording command was not accepted"
        )),
    }
}

fn recv_audio_control_ack<T>(ack_rx: Receiver<Result<T>>, label: &'static str) -> Result<T> {
    match ack_rx.recv_timeout(AUDIO_CONTROL_TIMEOUT) {
        Ok(result) => result,
        Err(err) => Err(match err {
            RecvTimeoutError::Timeout => {
                anyhow!("audio drain accepted {label} but did not acknowledge before timeout")
            }
            RecvTimeoutError::Disconnected => {
                anyhow!("audio drain accepted {label} but disconnected before acknowledging")
            }
        }),
    }
}

#[cfg(test)]
impl AudioHandle {
    /// Build an isolated audio handle for coordinator and buffer unit tests.
    ///
    /// # Returns
    ///
    /// A handle with an empty buffer, closed epoch, and default capture pipeline.
    pub(crate) fn test_handle() -> Self {
        Self {
            state: Arc::new(Mutex::new(CaptureState::new())),
            session_epoch: Arc::new(AtomicU64::new(0)),
            next_session_epoch: Arc::new(AtomicU64::new(0)),
            control: Arc::new(Mutex::new(None)),
        }
    }
}

/// Summary of the active microphone stream.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MicInfo {
    /// CPAL device name.
    pub name: String,
    /// Input stream sample rate opened from the OS/audio server.
    pub input_rate: u32,
    /// Number of input channels opened.
    pub channels: u16,
    /// CPAL sample format label.
    pub sample_format: String,
    /// PulseAudio/PipeWire source name when available.
    pub source_id: Option<String>,
    /// Whether the stream is resampled to the Parakeet target rate.
    pub resampling: bool,
    /// Human-readable note about why the opened input config was selected.
    pub config_note: Option<String>,
}

impl MicInfo {
    /// Return the concise startup/device-change label.
    ///
    /// # Returns
    ///
    /// A human-readable device summary.
    pub fn summary(&self) -> String {
        let channel_label = input_channel_label(self.channels);
        let rate_label = if self.resampling {
            format!(
                "{} Hz {} input -> {} Hz mono model",
                self.input_rate, channel_label, TARGET_RATE
            )
        } else if self.channels == 1 {
            format!("{} Hz input/model", self.input_rate)
        } else {
            format!(
                "{} Hz {} input -> mono model",
                self.input_rate, channel_label
            )
        };
        format!("{}, {}, {}", self.name, rate_label, self.sample_format)
    }

    /// Return detailed audio routing notes for verbose diagnostics.
    ///
    /// # Returns
    ///
    /// Lines describing the capture and model input shape.
    pub fn detail_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        lines.push(format!("model input: {} Hz mono PCM", TARGET_RATE));
        if self.channels > 1 {
            lines.push(format!(
                "capture path: CPAL opened {}; callback downmixes to mono before resampling",
                input_channel_label(self.channels)
            ));
        } else if self.resampling {
            lines.push("capture path: mono input, resampling to model rate".to_string());
        } else {
            lines.push("capture path: mono input, no resampling".to_string());
        }
        if let Some(note) = &self.config_note {
            lines.push(format!("input config: {note}"));
        }
        lines
    }

    /// Return whether this input appears to be a Bluetooth microphone.
    ///
    /// # Returns
    ///
    /// `true` when the source name or device label contains common Bluetooth
    /// identifiers.
    pub fn looks_bluetooth(&self) -> bool {
        is_bluetooth_input_name(&self.name)
            || self
                .source_id
                .as_deref()
                .is_some_and(is_bluetooth_input_name)
    }
}

fn input_channel_label(channels: u16) -> String {
    if channels == 1 {
        "mono".to_string()
    } else {
        format!("{channels}ch")
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
        let state = Arc::new(Mutex::new(CaptureState::new()));
        let session_epoch = Arc::new(AtomicU64::new(0));
        let current = Arc::new(Mutex::new(None));
        let alive = Arc::new(AtomicBool::new(true));
        let stream_error = Arc::new(Mutex::new(None));
        let control = Arc::new(Mutex::new(None));
        let (control_tx, control_rx) = bounded::<AudioControl>(4);
        *control.lock() = Some(control_tx);

        let handle = AudioHandle {
            state: Arc::clone(&state),
            session_epoch: Arc::clone(&session_epoch),
            next_session_epoch: Arc::new(AtomicU64::new(0)),
            control: Arc::clone(&control),
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
                    control,
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
    control: Arc<Mutex<Option<Sender<AudioControl>>>>,
    control_rx: Receiver<AudioControl>,
    log: Arc<Logger>,
    notifier: Notifier,
    ready: crossbeam_channel::Sender<Result<MicInfo>>,
}

fn audio_manager_loop(ctx: AudioManagerCtx) {
    let host = cpal::default_host();
    let idle_policy = idle_stream_policy();
    let mut live = match open_live_stream(
        &host,
        Arc::clone(&ctx.state),
        Arc::clone(&ctx.session_epoch),
        Arc::clone(&ctx.stream_error),
        idle_policy.paused_when_idle,
    ) {
        Ok(live) => {
            *ctx.current.lock() = Some(live.info.clone());
            let _ = ctx.ready.send(Ok(live.info.clone()));
            live
        }
        Err(err) => {
            let _ = ctx.ready.send(Err(err));
            return;
        }
    };

    while ctx.alive.load(Ordering::SeqCst) {
        crossbeam_channel::select! {
            recv(ctx.control_rx) -> msg => {
                match msg {
                    Ok(control) => {
                        handle_manager_control(control, &mut live, &ctx, idle_policy);
                        continue;
                    }
                    Err(_) => break,
                }
            }
            default(DEVICE_POLL_INTERVAL) => {}
        }

        if let Some(err) = ctx.stream_error.lock().take() {
            ctx.log
                .warn(format!("microphone stream failed ({err}); reopening"));
            ctx.notifier.microphone_unavailable(&err);
            drop(live);
            let Some(next_live) = reopen_until_success(&host, &ctx) else {
                break;
            };
            live = next_live;
            ctx.notifier.microphone_recovered(&live.info);
            continue;
        }

        let dropped_samples = live.dropped_samples.swap(0, Ordering::AcqRel);
        if dropped_samples != 0 {
            ctx.log.warn(format!(
                "microphone ring overflow dropped {dropped_samples} sample(s)"
            ));
        }

        if ctx.session_epoch.load(Ordering::Relaxed) != 0 {
            continue;
        }

        match selected_mic_identity(&host) {
            Ok(next) if next != live.identity => {
                let next_info = mic_info_from_identity(&next);
                ctx.log.verbose(format!(
                    "parakit: selected input changed from {} to {}",
                    live.info.summary(),
                    next_info.summary()
                ));
                drop(live);
                let Some(next_live) = reopen_until_success(&host, &ctx) else {
                    break;
                };
                live = next_live;
                ctx.log.mic_changed(&live.info);
            }
            Ok(_) => {}
            Err(err) => {
                ctx.log
                    .verbose(format!("parakit: input device scan failed: {err:#}"));
            }
        }
    }

    *ctx.control.lock() = None;
}

#[derive(Clone, Copy)]
struct IdleStreamPolicy {
    paused_when_idle: bool,
}

fn idle_stream_policy() -> IdleStreamPolicy {
    IdleStreamPolicy {
        paused_when_idle: cfg!(target_os = "windows"),
    }
}

fn handle_manager_control(
    control: AudioControl,
    live: &mut LiveStream,
    ctx: &AudioManagerCtx,
    idle_policy: IdleStreamPolicy,
) {
    match control {
        AudioControl::Start { epoch, ack } => {
            match start_live_recording(live, epoch, idle_policy) {
                Ok(()) => {
                    if ack.send(Ok(())).is_err() {
                        rollback_abandoned_start(live, ctx, idle_policy);
                    }
                }
                Err(err) => {
                    let _ = ack.send(Err(err));
                }
            }
        }
        AudioControl::Stop { ack } => {
            let result = stop_live_recording(live);
            match result {
                Ok(pcm) => {
                    let _ = ack.send(Ok(pcm));
                    if let Err(err) = pause_live_stream(live, idle_policy) {
                        ctx.log
                            .warn(format!("could not pause idle microphone stream: {err:#}"));
                    }
                }
                Err(err) => {
                    let _ = ack.send(Err(err));
                }
            }
        }
    }
}

fn rollback_abandoned_start(
    live: &mut LiveStream,
    ctx: &AudioManagerCtx,
    idle_policy: IdleStreamPolicy,
) {
    ctx.log.warn(
        "recording start completed after the caller gave up; stopping abandoned capture"
            .to_string(),
    );
    if let Err(err) = stop_live_recording(live) {
        ctx.log.warn(format!(
            "could not stop abandoned microphone recording: {err:#}"
        ));
    }
    if let Err(err) = pause_live_stream(live, idle_policy) {
        ctx.log.warn(format!(
            "could not pause abandoned microphone stream: {err:#}"
        ));
    }
}

fn start_live_recording(
    live: &mut LiveStream,
    epoch: u64,
    idle_policy: IdleStreamPolicy,
) -> Result<()> {
    if live.paused {
        start_audio_drain(live, epoch, false)?;
        if let Err(err) = live.stream.play().context("stream.play() failed") {
            let _ = stop_live_recording(live);
            return Err(err);
        }
        live.paused = false;
        return Ok(());
    }

    start_audio_drain(live, epoch, !idle_policy.paused_when_idle)
}

fn start_audio_drain(live: &mut LiveStream, epoch: u64, include_pre_roll: bool) -> Result<()> {
    let (ack_tx, ack_rx) = bounded(1);
    live.drain_control
        .send(DrainControl::Start {
            epoch,
            include_pre_roll,
            ack: ack_tx,
        })
        .context("audio drain is not available")?;
    recv_drain_control_ack(ack_rx, "Start")
}

fn stop_live_recording(live: &mut LiveStream) -> Result<Vec<f32>> {
    let (ack_tx, ack_rx) = bounded(1);
    live.drain_control
        .send(DrainControl::Stop { ack: ack_tx })
        .context("audio drain is not available")?;
    recv_drain_control_ack(ack_rx, "Stop")
}

fn pause_live_stream(live: &mut LiveStream, idle_policy: IdleStreamPolicy) -> Result<()> {
    if !idle_policy.paused_when_idle || live.paused {
        return Ok(());
    }
    live.stream.pause().context("stream.pause() failed")?;
    live.paused = true;
    Ok(())
}

fn recv_drain_control_ack<T>(ack_rx: Receiver<T>, label: &'static str) -> Result<T> {
    ack_rx
        .recv_timeout(AUDIO_CONTROL_TIMEOUT)
        .map_err(|err| match err {
            RecvTimeoutError::Timeout => {
                anyhow!("audio drain accepted {label} but did not acknowledge before timeout")
            }
            RecvTimeoutError::Disconnected => {
                anyhow!("audio drain accepted {label} but disconnected before acknowledging")
            }
        })
}

fn reopen_until_success(host: &cpal::Host, ctx: &AudioManagerCtx) -> Option<LiveStream> {
    let mut retry_delay = DEVICE_POLL_INTERVAL;
    let mut attempts = 0_u32;
    while ctx.alive.load(Ordering::SeqCst) {
        match open_live_stream(
            host,
            Arc::clone(&ctx.state),
            Arc::clone(&ctx.session_epoch),
            Arc::clone(&ctx.stream_error),
            idle_stream_policy().paused_when_idle,
        ) {
            Ok(live) => {
                *ctx.current.lock() = Some(live.info.clone());
                return Some(live);
            }
            Err(err) => {
                attempts = attempts.saturating_add(1);
                *ctx.current.lock() = None;
                if attempts == 1 || attempts.is_multiple_of(30) {
                    ctx.log
                        .warn(format!("no usable microphone yet ({err:#}); retrying"));
                } else {
                    ctx.log
                        .verbose(format!("microphone still unavailable ({err:#})"));
                }
                if !sleep_while_alive(&ctx.alive, retry_delay) {
                    return None;
                }
                retry_delay = (retry_delay * 2).min(DEVICE_RETRY_MAX_INTERVAL);
            }
        }
    }
    None
}

fn sleep_while_alive(alive: &AtomicBool, duration: Duration) -> bool {
    let mut slept = Duration::ZERO;
    while slept < duration {
        if !alive.load(Ordering::SeqCst) {
            return false;
        }
        let step = (duration - slept).min(Duration::from_millis(100));
        thread::sleep(step);
        slept += step;
    }
    alive.load(Ordering::SeqCst)
}

struct LiveStream {
    info: MicInfo,
    identity: MicIdentity,
    drain_control: Sender<DrainControl>,
    stream: Stream,
    paused: bool,
    dropped_samples: Arc<AtomicU64>,
    _drain: AudioDrain,
}

enum AudioControl {
    Start { epoch: u64, ack: Sender<Result<()>> },
    Stop { ack: Sender<Result<Vec<f32>>> },
}

enum DrainControl {
    Start {
        epoch: u64,
        include_pre_roll: bool,
        ack: Sender<()>,
    },
    Stop {
        ack: Sender<Vec<f32>>,
    },
}

struct AudioDrain {
    alive: Arc<AtomicBool>,
    wake: Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for AudioDrain {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Release);
        let _ = self.wake.try_send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
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
    let host = cpal::default_host();
    selected_mic_info(&host)
}

fn open_live_stream(
    host: &cpal::Host,
    state: Arc<Mutex<CaptureState>>,
    session_epoch: Arc<AtomicU64>,
    stream_error: Arc<Mutex<Option<String>>>,
    start_paused: bool,
) -> Result<LiveStream> {
    let selected = select_input_device(host)?;
    let (info, identity) = mic_snapshot_from_selected(&selected);

    let hw_rate = selected.config.sample_rate().0;
    let channels = selected.config.channels() as usize;
    let stream_config: StreamConfig = selected.config.config();
    let pipeline = CapturePipeline {
        resampler: make_resampler(hw_rate)?,
    };
    let ring = HeapRb::<f32>::new(audio_ring_capacity(hw_rate));
    let (producer, consumer) = ring.split();
    let (wake_tx, wake_rx) = bounded::<()>(1);
    let (control_tx, control_rx) = bounded::<DrainControl>(4);
    let stream_alive = Arc::new(AtomicBool::new(true));
    let dropped_samples = Arc::new(AtomicU64::new(0));
    let drain = spawn_audio_drain(
        consumer,
        wake_rx,
        control_rx,
        Arc::clone(&state),
        Arc::clone(&session_epoch),
        pipeline,
        Arc::clone(&stream_alive),
    )
    .context("spawn audio drain")?;
    let drain = AudioDrain {
        alive: stream_alive,
        wake: wake_tx.clone(),
        thread: Some(drain),
    };

    let stream = match selected.config.sample_format() {
        SampleFormat::I8 => build_stream::<i8>(
            &selected.device,
            &stream_config,
            channels,
            producer,
            stream_error,
            Arc::clone(&dropped_samples),
            wake_tx.clone(),
        )?,
        SampleFormat::I16 => build_stream::<i16>(
            &selected.device,
            &stream_config,
            channels,
            producer,
            stream_error,
            Arc::clone(&dropped_samples),
            wake_tx.clone(),
        )?,
        SampleFormat::I32 => build_stream::<i32>(
            &selected.device,
            &stream_config,
            channels,
            producer,
            stream_error,
            Arc::clone(&dropped_samples),
            wake_tx.clone(),
        )?,
        SampleFormat::U8 => build_stream::<u8>(
            &selected.device,
            &stream_config,
            channels,
            producer,
            stream_error,
            Arc::clone(&dropped_samples),
            wake_tx.clone(),
        )?,
        SampleFormat::U16 => build_stream::<u16>(
            &selected.device,
            &stream_config,
            channels,
            producer,
            stream_error,
            Arc::clone(&dropped_samples),
            wake_tx.clone(),
        )?,
        SampleFormat::U32 => build_stream::<u32>(
            &selected.device,
            &stream_config,
            channels,
            producer,
            stream_error,
            Arc::clone(&dropped_samples),
            wake_tx.clone(),
        )?,
        SampleFormat::F32 => build_stream::<f32>(
            &selected.device,
            &stream_config,
            channels,
            producer,
            stream_error,
            Arc::clone(&dropped_samples),
            wake_tx.clone(),
        )?,
        SampleFormat::F64 => build_stream::<f64>(
            &selected.device,
            &stream_config,
            channels,
            producer,
            stream_error,
            Arc::clone(&dropped_samples),
            wake_tx.clone(),
        )?,
        other => return Err(anyhow!("unsupported sample format: {:?}", other)),
    };

    if !start_paused {
        stream.play().context("stream.play() failed")?;
    }
    Ok(LiveStream {
        info,
        identity,
        drain_control: control_tx,
        stream,
        paused: start_paused,
        dropped_samples,
        _drain: drain,
    })
}

fn audio_ring_capacity(hw_rate: u32) -> usize {
    (hw_rate as usize * AUDIO_RING_SECONDS).max(AUDIO_RING_MIN_CAPACITY)
}

fn callback_scratch_frames(config: &StreamConfig) -> usize {
    match config.buffer_size {
        cpal::BufferSize::Fixed(frames) => frames as usize,
        cpal::BufferSize::Default => DEFAULT_CALLBACK_SCRATCH_FRAMES,
    }
    .max(1)
}

fn spawn_audio_drain(
    consumer: HeapCons<f32>,
    wake_rx: Receiver<()>,
    control_rx: Receiver<DrainControl>,
    state: Arc<Mutex<CaptureState>>,
    session_epoch: Arc<AtomicU64>,
    pipeline: CapturePipeline,
    alive: Arc<AtomicBool>,
) -> std::io::Result<JoinHandle<()>> {
    thread::Builder::new()
        .name("parakit-audio-drain".into())
        .spawn(move || {
            audio_drain_loop(
                consumer,
                wake_rx,
                control_rx,
                state,
                session_epoch,
                pipeline,
                alive,
            )
        })
}

fn audio_drain_loop(
    mut consumer: HeapCons<f32>,
    wake_rx: Receiver<()>,
    control_rx: Receiver<DrainControl>,
    state: Arc<Mutex<CaptureState>>,
    session_epoch: Arc<AtomicU64>,
    mut pipeline: CapturePipeline,
    alive: Arc<AtomicBool>,
) {
    let mut input = vec![0.0_f32; 8192];
    let mut resampled = Vec::with_capacity(8192);
    while alive.load(Ordering::Acquire) {
        while let Ok(control) = control_rx.try_recv() {
            handle_audio_control(
                control,
                &mut consumer,
                &state,
                &session_epoch,
                &mut pipeline,
                &mut input,
                &mut resampled,
            );
        }
        let drained_any = drain_audio_ring(
            &mut consumer,
            &state,
            &session_epoch,
            &mut pipeline,
            &mut input,
            &mut resampled,
        );
        if !drained_any {
            crossbeam_channel::select! {
                recv(control_rx) -> msg => {
                    match msg {
                        Ok(control) => handle_audio_control(
                            control,
                            &mut consumer,
                            &state,
                            &session_epoch,
                            &mut pipeline,
                            &mut input,
                            &mut resampled,
                        ),
                        Err(_) => break,
                    }
                }
                recv(wake_rx) -> _ => {}
            }
        }
    }
}

fn handle_audio_control(
    control: DrainControl,
    consumer: &mut HeapCons<f32>,
    state: &Mutex<CaptureState>,
    session_epoch: &AtomicU64,
    pipeline: &mut CapturePipeline,
    input: &mut [f32],
    resampled: &mut Vec<f32>,
) {
    match control {
        DrainControl::Start {
            epoch,
            include_pre_roll,
            ack,
        } => {
            if include_pre_roll {
                while drain_audio_ring(consumer, state, session_epoch, pipeline, input, resampled) {
                }
            } else {
                discard_audio_ring(consumer, input);
            }
            pipeline.reset_recording();
            if include_pre_roll {
                state.lock().begin_recording();
            } else {
                state.lock().begin_recording_without_pre_roll();
            }
            session_epoch.store(epoch, Ordering::Release);
            let _ = ack.send(());
        }
        DrainControl::Stop { ack } => {
            while drain_audio_ring(consumer, state, session_epoch, pipeline, input, resampled) {}
            resampled.clear();
            pipeline.finish_recording(resampled);
            if !resampled.is_empty() {
                append_processed_samples(state, session_epoch, resampled);
            }
            resampled.clear();
            session_epoch.store(0, Ordering::Release);
            let pcm = state.lock().take_recording();
            pipeline.reset_recording();
            let _ = ack.send(pcm);
        }
    }
}

fn discard_audio_ring(consumer: &mut HeapCons<f32>, input: &mut [f32]) {
    while consumer.pop_slice(input) != 0 {}
}

fn drain_audio_ring(
    consumer: &mut HeapCons<f32>,
    state: &Mutex<CaptureState>,
    session_epoch: &AtomicU64,
    pipeline: &mut CapturePipeline,
    input: &mut [f32],
    resampled: &mut Vec<f32>,
) -> bool {
    let mut drained_any = false;
    loop {
        let n = consumer.pop_slice(input);
        if n == 0 {
            break;
        }
        drained_any = true;
        let processed = pipeline.process(&input[..n], resampled);
        if !processed.is_empty() {
            append_processed_samples(state, session_epoch, processed);
        }
    }
    drained_any
}

fn make_resampler(hw_rate: u32) -> Result<Option<ResamplerState>> {
    if hw_rate == TARGET_RATE {
        return Ok(None);
    }

    let resampler = SincFixedIn::<f32>::new(
        TARGET_RATE as f64 / hw_rate as f64,
        2.0,
        resampler_params(),
        RESAMPLE_CHUNK_SIZE,
        1,
    )
    .context("failed to construct resampler")?;
    Ok(Some(ResamplerState::new(resampler, RESAMPLE_CHUNK_SIZE)))
}

struct SelectedInput {
    device: cpal::Device,
    name: String,
    config: cpal::SupportedStreamConfig,
    config_note: Option<String>,
    is_default: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MicIdentity {
    name: String,
    source_id: Option<String>,
    input_rate: u32,
    channels: u16,
    sample_format: String,
}

fn select_input_device(host: &cpal::Host) -> Result<SelectedInput> {
    let default_device = host.default_input_device();
    let default_name = default_device.as_ref().and_then(|d| d.name().ok());
    if let Some(device) = default_device {
        let name = default_name
            .clone()
            .unwrap_or_else(|| "<default input>".to_string());
        if !is_virtual_input_name(&name) {
            if let Ok(config) = device.default_input_config() {
                let (config, config_note) = select_preferred_input_config(&device, config);
                return Ok(SelectedInput {
                    device,
                    name,
                    config,
                    config_note,
                    is_default: true,
                });
            }
        }
    }

    let mut physical = Vec::new();
    let mut virtual_inputs = Vec::new();

    for device in host
        .input_devices()
        .context("failed to list input devices")?
    {
        let name = device
            .name()
            .unwrap_or_else(|_| "<unknown input>".to_string());
        let (config, config_note) = match device.default_input_config() {
            Ok(config) => select_preferred_input_config(&device, config),
            Err(_) => continue,
        };
        let selected = SelectedInput {
            device,
            name: name.clone(),
            config,
            config_note,
            is_default: default_name.as_deref() == Some(name.as_str()),
        };
        if is_virtual_input_name(&name) {
            virtual_inputs.push(selected);
        } else {
            physical.push(selected);
        }
    }

    if let Some(default_name) = default_name.as_deref() {
        if !is_virtual_input_name(default_name) {
            if let Some(index) = physical.iter().position(|d| d.name == default_name) {
                return Ok(physical.swap_remove(index));
            }
        }
    }

    if let Some(device) = physical.into_iter().next() {
        return Ok(device);
    }

    if let Some(default_name) = default_name {
        if let Some(index) = virtual_inputs.iter().position(|d| d.name == default_name) {
            return Ok(virtual_inputs.swap_remove(index));
        }
    }

    virtual_inputs
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no usable input device"))
}

fn select_preferred_input_config(
    device: &cpal::Device,
    default_config: cpal::SupportedStreamConfig,
) -> (cpal::SupportedStreamConfig, Option<String>) {
    if default_config.channels() == 1 {
        return (default_config, None);
    }

    let default_channels = default_config.channels();
    let ranges = match device.supported_input_configs() {
        Ok(ranges) => ranges,
        Err(err) => {
            return (
                default_config,
                Some(format!(
                    "could not inspect alternate input configs ({err}); downmixing {default_channels}ch input to mono"
                )),
            );
        }
    };
    let ranges = ranges.collect::<Vec<_>>();

    match preferred_mono_config_from_ranges(&default_config, ranges.iter()) {
        Some(config) => (
            config,
            Some(format!(
                "selected same-rate/same-format mono input instead of {default_channels}ch default"
            )),
        ),
        None => {
            let mut note = format!(
                "no same-rate/same-format mono input config advertised; downmixing {default_channels}ch input to mono"
            );
            if let Some(alternate) = lower_cost_mono_config_note(&default_config, &ranges) {
                note.push_str("; ");
                note.push_str(&alternate);
            }
            (default_config, Some(note))
        }
    }
}

fn preferred_mono_config_from_ranges<'a, I>(
    default_config: &cpal::SupportedStreamConfig,
    ranges: I,
) -> Option<cpal::SupportedStreamConfig>
where
    I: IntoIterator<Item = &'a cpal::SupportedStreamConfigRange>,
{
    let default_rate = default_config.sample_rate();
    let default_format = default_config.sample_format();

    ranges.into_iter().find_map(|range| {
        if range.channels() == 1 && range.sample_format() == default_format {
            range.try_with_sample_rate(default_rate)
        } else {
            None
        }
    })
}

fn lower_cost_mono_config_note(
    default_config: &cpal::SupportedStreamConfig,
    ranges: &[cpal::SupportedStreamConfigRange],
) -> Option<String> {
    let default_rate = default_config.sample_rate();
    let default_format = default_config.sample_format();
    let same_rate_other_format = ranges.iter().find_map(|range| {
        if range.channels() == 1 && range.sample_format() != default_format {
            range.try_with_sample_rate(default_rate)
        } else {
            None
        }
    });
    if let Some(config) = same_rate_other_format {
        return Some(format!(
            "same-rate mono is available as {:?}, but not selected because it changes sample format",
            config.sample_format()
        ));
    }

    let target_rate = cpal::SampleRate(TARGET_RATE);
    let target_rate_mono = ranges.iter().find_map(|range| {
        if range.channels() == 1 {
            range.try_with_sample_rate(target_rate)
        } else {
            None
        }
    });
    target_rate_mono.map(|config| {
        format!(
            "{} Hz mono is available as {:?}, but not selected because the current policy preserves the OS default sample rate",
            TARGET_RATE,
            config.sample_format()
        )
    })
}

fn selected_mic_info(host: &cpal::Host) -> Result<MicInfo> {
    let selected = select_input_device(host)?;
    Ok(mic_snapshot_from_selected(&selected).0)
}

fn selected_mic_identity(host: &cpal::Host) -> Result<MicIdentity> {
    let selected = select_input_device(host)?;
    Ok(polled_mic_identity_from_selected(&selected))
}

fn mic_snapshot_from_selected(selected: &SelectedInput) -> (MicInfo, MicIdentity) {
    let raw_identity = raw_mic_identity_from_selected(selected);
    let mut info = mic_info_from_identity(&raw_identity);
    enhance_mic_info(&mut info, selected.is_default);
    info.config_note = selected.config_note.clone();
    if info.source_id.is_none() {
        info.source_id = default_source_id_for_identity(selected);
    }
    let identity = source_aware_mic_identity(raw_identity, info.source_id.clone());
    (info, identity)
}

fn raw_mic_identity_from_selected(selected: &SelectedInput) -> MicIdentity {
    mic_identity_from_config(&selected.name, &selected.config)
}

fn polled_mic_identity_from_selected(selected: &SelectedInput) -> MicIdentity {
    let raw_identity = raw_mic_identity_from_selected(selected);
    source_aware_mic_identity(raw_identity, default_source_id_for_identity(selected))
}

fn source_aware_mic_identity(mut identity: MicIdentity, source_id: Option<String>) -> MicIdentity {
    identity.source_id = source_id;
    identity
}

#[cfg(target_os = "linux")]
fn default_source_id_for_identity(selected: &SelectedInput) -> Option<String> {
    if !selected.is_default && selected.name != "default" {
        return None;
    }
    pactl_default_source_name()
}

#[cfg(not(target_os = "linux"))]
fn default_source_id_for_identity(_selected: &SelectedInput) -> Option<String> {
    None
}

fn mic_identity_from_config(name: &str, config: &cpal::SupportedStreamConfig) -> MicIdentity {
    MicIdentity {
        name: name.to_string(),
        source_id: None,
        input_rate: config.sample_rate().0,
        channels: config.channels(),
        sample_format: format!("{:?}", config.sample_format()),
    }
}

fn mic_info_from_identity(identity: &MicIdentity) -> MicInfo {
    MicInfo {
        name: identity.name.clone(),
        input_rate: identity.input_rate,
        channels: identity.channels,
        sample_format: identity.sample_format.clone(),
        source_id: identity.source_id.clone(),
        resampling: identity.input_rate != TARGET_RATE,
        config_note: None,
    }
}

#[cfg(target_os = "linux")]
fn enhance_mic_info(info: &mut MicInfo, is_default: bool) {
    if !is_default && info.name != "default" {
        return;
    }
    let Some(source) = pactl_default_source_info() else {
        return;
    };
    let source_id = source.name.clone();
    info.name = source.description.unwrap_or(source.name);
    if let Some(rate) = source.rate {
        info.input_rate = rate;
    }
    if let Some(channels) = source.channels {
        info.channels = channels;
    }
    if let Some(format) = source.sample_format {
        info.sample_format = format;
    }
    info.source_id = Some(source_id);
    info.resampling = info.input_rate != TARGET_RATE;
}

#[cfg(not(target_os = "linux"))]
fn enhance_mic_info(_info: &mut MicInfo, _is_default: bool) {}

/// Return whether a device name looks like a monitor or virtual input.
///
/// # Returns
///
/// `true` for names parakit should avoid unless no physical-looking input is
/// available.
pub fn is_virtual_input_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    let patterns = [
        "monitor of",
        ".monitor",
        " monitor",
        "loopback",
        "virtual",
        "null",
        "dummy",
        "blackhole",
        "soundflower",
        "stereo mix",
        "what u hear",
        "wasapi output",
    ];
    patterns.iter().any(|pattern| lower.contains(pattern))
}

/// Return whether an input name or source id looks like a Bluetooth microphone.
///
/// # Returns
///
/// `true` for common Bluetooth transport, profile, and headset labels.
pub fn is_bluetooth_input_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    let patterns = [
        "bluetooth",
        "bluez",
        "headset_head_unit",
        "headset-head-unit",
        "handsfree",
        "hands-free",
        "hands free",
        "hfp",
        "hsp",
        "a2dp",
        "airpod",
        "earbud",
        "earbuds",
        "galaxy buds",
        "pixel buds",
        "freebuds",
    ];
    patterns.iter().any(|pattern| lower.contains(pattern))
}

#[derive(Default)]
struct CapturePipeline {
    resampler: Option<ResamplerState>,
}

impl CapturePipeline {
    fn reset_recording(&mut self) {
        if let Some(resampler) = &mut self.resampler {
            resampler.reset_recording();
        }
    }

    fn process<'a>(&mut self, input: &'a [f32], out: &'a mut Vec<f32>) -> &'a [f32] {
        match &mut self.resampler {
            Some(resampler) => {
                out.clear();
                resampler.process(input, out);
                out
            }
            None => input,
        }
    }

    fn finish_recording(&mut self, out: &mut Vec<f32>) {
        if let Some(resampler) = &mut self.resampler {
            resampler.flush_recording(out);
        }
    }
}

/// Per-stream state for resampling one recording at a time.
struct ResamplerState {
    resampler: SincFixedIn<f32>,
    scratch: Vec<f32>,
    input_buf: Vec<Vec<f32>>,
    output_buf: Vec<Vec<f32>>,
    chunk_size: usize,
}

impl ResamplerState {
    fn new(resampler: SincFixedIn<f32>, chunk_size: usize) -> Self {
        let output_len = resampler.output_frames_max();
        Self {
            resampler,
            scratch: Vec::with_capacity(chunk_size * 4),
            input_buf: vec![vec![0.0; chunk_size]],
            output_buf: vec![vec![0.0; output_len]],
            chunk_size,
        }
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.scratch.extend_from_slice(input);
        let mut processed = 0;
        while self.scratch.len().saturating_sub(processed) >= self.chunk_size {
            self.input_buf[0]
                .copy_from_slice(&self.scratch[processed..processed + self.chunk_size]);
            self.process_chunk(out);
            processed += self.chunk_size;
        }
        if processed > 0 {
            let remaining = self.scratch.len() - processed;
            if remaining == 0 {
                self.scratch.clear();
            } else {
                self.scratch.copy_within(processed.., 0);
                self.scratch.truncate(remaining);
            }
        }
    }

    fn flush_recording(&mut self, out: &mut Vec<f32>) {
        if !self.scratch.is_empty() {
            debug_assert!(self.scratch.len() < self.chunk_size);
            self.input_buf[0].fill(0.0);
            self.input_buf[0][..self.scratch.len()].copy_from_slice(&self.scratch);
            self.scratch.clear();
            self.process_chunk(out);
        }
        self.resampler.reset();
    }

    fn reset_recording(&mut self) {
        self.scratch.clear();
        self.resampler.reset();
    }

    fn process_chunk(&mut self, out: &mut Vec<f32>) {
        match self
            .resampler
            .process_into_buffer(&self.input_buf, &mut self.output_buf, None)
        {
            Ok((_, written)) => {
                if let Some(ch0) = self.output_buf.first() {
                    out.extend_from_slice(&ch0[..written]);
                }
            }
            Err(e) => {
                eprintln!("parakit: resampler error (dropped chunk): {e}");
            }
        }
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    mut producer: HeapProd<f32>,
    stream_error: Arc<Mutex<Option<String>>>,
    dropped_samples: Arc<AtomicU64>,
    wake: Sender<()>,
) -> Result<Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32> + 'static,
    f32: cpal::FromSample<T>,
{
    let mut mono_scratch = vec![0.0_f32; callback_scratch_frames(config)];
    let err_state = Arc::clone(&stream_error);
    let err_fn = move |err: cpal::StreamError| {
        *err_state.lock() = Some(err.to_string());
    };

    let stream = device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let frame_count = if channels == 1 {
                    data.len()
                } else {
                    data.len() / channels
                };
                let mut frame_offset = 0;

                while frame_offset < frame_count {
                    let chunk_frames = (frame_count - frame_offset).min(mono_scratch.len());
                    if channels == 1 {
                        for (i, slot) in mono_scratch.iter_mut().take(chunk_frames).enumerate() {
                            *slot = cpal::Sample::from_sample(data[frame_offset + i]);
                        }
                    } else {
                        for (i, slot) in mono_scratch.iter_mut().take(chunk_frames).enumerate() {
                            let frame = frame_offset + i;
                            let mut sum = 0.0f32;
                            for c in 0..channels {
                                let s: f32 = cpal::Sample::from_sample(data[frame * channels + c]);
                                sum += s;
                            }
                            *slot = sum / channels as f32;
                        }
                    }

                    let written = producer.push_slice(&mono_scratch[..chunk_frames]);
                    if written < chunk_frames {
                        dropped_samples
                            .fetch_add((chunk_frames - written) as u64, Ordering::Relaxed);
                    }
                    frame_offset += chunk_frames;
                }

                let _ = wake.try_send(());
            },
            err_fn,
            None,
        )
        .context("failed to build input stream")?;

    Ok(stream)
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
#[path = "audio_manager_tests.rs"]
mod audio_manager_tests;
