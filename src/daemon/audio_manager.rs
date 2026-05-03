//! Microphone capture into a shared f32 buffer at 16 kHz mono.
//!
//! The CPAL stream is owned by a dedicated manager thread. That keeps stream
//! creation and teardown on one thread, lets the daemon reopen the stream when
//! the OS default input changes, and avoids crashing when a USB or Bluetooth
//! microphone disappears.

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use crossbeam_channel::{bounded, Receiver, Sender};
use parakit::audio_file::resampler_params;
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
use crate::daemon::audio_pactl::pactl_default_source_info;

pub use parakit::constants::TARGET_RATE;

/// Reusable capacity for ordinary dictation bursts.
const RECORDING_CAPACITY: usize = TARGET_RATE as usize * 90;
/// Hard cap for one held recording to prevent unbounded memory growth.
const MAX_RECORDING_SAMPLES: usize = TARGET_RATE as usize * 60 * 5;
const PRE_ROLL_SAMPLES: usize = TARGET_RATE as usize * 350 / 1000;
const AUDIO_RING_SECONDS: usize = 6;
const AUDIO_RING_MIN_CAPACITY: usize = TARGET_RATE as usize * AUDIO_RING_SECONDS;
const AUDIO_DRAIN_IDLE_SLEEP: Duration = Duration::from_millis(5);
const AUDIO_CONTROL_TIMEOUT: Duration = Duration::from_secs(1);
const DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(2);
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
    pub fn start_recording(&self) {
        let next = self
            .next_session_epoch
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1)
            .max(1);
        if self.send_control_start(next) {
            return;
        }

        let mut state = self.state.lock();
        state.begin_recording();
        self.session_epoch.store(next, Ordering::Release);
    }

    /// Stop recording and take ownership of the buffered samples.
    ///
    /// # Returns
    ///
    /// The captured mono PCM samples at [`TARGET_RATE`].
    pub fn stop_recording(&self) -> Vec<f32> {
        if let Some(pcm) = self.send_control_stop() {
            return pcm;
        }

        self.session_epoch.store(0, Ordering::Release);
        self.state.lock().take_recording()
    }

    fn send_control_start(&self, epoch: u64) -> bool {
        let Some(control) = self.control.lock().clone() else {
            return false;
        };
        let (ack_tx, ack_rx) = bounded(1);
        if control
            .send(AudioControl::Start { epoch, ack: ack_tx })
            .is_err()
        {
            return false;
        }
        ack_rx.recv_timeout(AUDIO_CONTROL_TIMEOUT).is_ok()
    }

    fn send_control_stop(&self) -> Option<Vec<f32>> {
        let control = self.control.lock().clone()?;
        let (ack_tx, ack_rx) = bounded(1);
        if control.send(AudioControl::Stop { ack: ack_tx }).is_err() {
            return None;
        }
        ack_rx.recv_timeout(AUDIO_CONTROL_TIMEOUT).ok()
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
}

impl MicInfo {
    /// Return the concise startup/device-change label.
    ///
    /// # Returns
    ///
    /// A human-readable device summary.
    pub fn summary(&self) -> String {
        let channel_label = if self.channels == 1 {
            "mono".to_string()
        } else {
            format!("{}ch", self.channels)
        };
        let rate_label = if self.resampling {
            format!("{} Hz input -> {} Hz model", self.input_rate, TARGET_RATE)
        } else {
            format!("{} Hz input/model", self.input_rate)
        };
        format!(
            "{}, {}, {}, {}",
            self.name, rate_label, channel_label, self.sample_format
        )
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
        if self.buffer.capacity() < RECORDING_CAPACITY {
            self.buffer
                .reserve_exact(RECORDING_CAPACITY - self.buffer.capacity());
        }
        self.buffer.clear();
        self.buffer.extend(self.pre_roll.iter().copied());
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
    log: Arc<Logger>,
    notifier: Notifier,
    ready: crossbeam_channel::Sender<Result<MicInfo>>,
}

fn audio_manager_loop(ctx: AudioManagerCtx) {
    let host = cpal::default_host();
    let mut live = match open_live_stream(
        &host,
        Arc::clone(&ctx.state),
        Arc::clone(&ctx.session_epoch),
        Arc::clone(&ctx.stream_error),
    ) {
        Ok(live) => {
            *ctx.current.lock() = Some(live.info.clone());
            *ctx.control.lock() = Some(live.control.clone());
            let _ = ctx.ready.send(Ok(live.info.clone()));
            live
        }
        Err(err) => {
            let _ = ctx.ready.send(Err(err));
            return;
        }
    };

    while ctx.alive.load(Ordering::SeqCst) {
        thread::sleep(DEVICE_POLL_INTERVAL);

        if let Some(err) = ctx.stream_error.lock().take() {
            ctx.log
                .warn(format!("microphone stream failed ({err}); reopening"));
            ctx.notifier.microphone_unavailable(&err);
            *ctx.control.lock() = None;
            drop(live);
            let Some(next_live) = reopen_until_success(&host, &ctx) else {
                break;
            };
            live = next_live;
            *ctx.control.lock() = Some(live.control.clone());
            ctx.notifier.microphone_recovered(&live.info);
            continue;
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
                *ctx.control.lock() = None;
                drop(live);
                let Some(next_live) = reopen_until_success(&host, &ctx) else {
                    break;
                };
                live = next_live;
                *ctx.control.lock() = Some(live.control.clone());
                ctx.log.mic_changed(&live.info);
            }
            Ok(_) => {}
            Err(err) => {
                ctx.log
                    .verbose(format!("parakit: input device scan failed: {err:#}"));
            }
        }
    }
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
        ) {
            Ok(live) => {
                *ctx.current.lock() = Some(live.info.clone());
                *ctx.control.lock() = Some(live.control.clone());
                return Some(live);
            }
            Err(err) => {
                attempts = attempts.saturating_add(1);
                *ctx.current.lock() = None;
                *ctx.control.lock() = None;
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
    control: Sender<AudioControl>,
    _stream: Stream,
    _drain: AudioDrain,
}

enum AudioControl {
    Start { epoch: u64, ack: Sender<()> },
    Stop { ack: Sender<Vec<f32>> },
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
    let (control_tx, control_rx) = bounded::<AudioControl>(4);
    let stream_alive = Arc::new(AtomicBool::new(true));
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
            wake_tx.clone(),
        )?,
        SampleFormat::I16 => build_stream::<i16>(
            &selected.device,
            &stream_config,
            channels,
            producer,
            stream_error,
            wake_tx.clone(),
        )?,
        SampleFormat::I32 => build_stream::<i32>(
            &selected.device,
            &stream_config,
            channels,
            producer,
            stream_error,
            wake_tx.clone(),
        )?,
        SampleFormat::U8 => build_stream::<u8>(
            &selected.device,
            &stream_config,
            channels,
            producer,
            stream_error,
            wake_tx.clone(),
        )?,
        SampleFormat::U16 => build_stream::<u16>(
            &selected.device,
            &stream_config,
            channels,
            producer,
            stream_error,
            wake_tx.clone(),
        )?,
        SampleFormat::U32 => build_stream::<u32>(
            &selected.device,
            &stream_config,
            channels,
            producer,
            stream_error,
            wake_tx.clone(),
        )?,
        SampleFormat::F32 => build_stream::<f32>(
            &selected.device,
            &stream_config,
            channels,
            producer,
            stream_error,
            wake_tx.clone(),
        )?,
        SampleFormat::F64 => build_stream::<f64>(
            &selected.device,
            &stream_config,
            channels,
            producer,
            stream_error,
            wake_tx.clone(),
        )?,
        other => return Err(anyhow!("unsupported sample format: {:?}", other)),
    };

    stream.play().context("stream.play() failed")?;
    Ok(LiveStream {
        info,
        identity,
        control: control_tx,
        _stream: stream,
        _drain: drain,
    })
}

fn audio_ring_capacity(hw_rate: u32) -> usize {
    (hw_rate as usize * AUDIO_RING_SECONDS).max(AUDIO_RING_MIN_CAPACITY)
}

fn spawn_audio_drain(
    consumer: HeapCons<f32>,
    wake_rx: Receiver<()>,
    control_rx: Receiver<AudioControl>,
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
    control_rx: Receiver<AudioControl>,
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
                default(AUDIO_DRAIN_IDLE_SLEEP) => {}
            }
        }
    }
}

fn handle_audio_control(
    control: AudioControl,
    consumer: &mut HeapCons<f32>,
    state: &Mutex<CaptureState>,
    session_epoch: &AtomicU64,
    pipeline: &mut CapturePipeline,
    input: &mut [f32],
    resampled: &mut Vec<f32>,
) {
    match control {
        AudioControl::Start { epoch, ack } => {
            while drain_audio_ring(consumer, state, session_epoch, pipeline, input, resampled) {}
            pipeline.reset_recording();
            state.lock().begin_recording();
            session_epoch.store(epoch, Ordering::Release);
            let _ = ack.send(());
        }
        AudioControl::Stop { ack } => {
            while drain_audio_ring(consumer, state, session_epoch, pipeline, input, resampled) {}
            let mut flushed = Vec::new();
            pipeline.finish_recording(&mut flushed);
            if !flushed.is_empty() {
                append_processed_samples(state, session_epoch, &flushed);
            }
            session_epoch.store(0, Ordering::Release);
            let pcm = state.lock().take_recording();
            while drain_audio_ring(consumer, state, session_epoch, pipeline, input, resampled) {}
            pipeline.reset_recording();
            let _ = ack.send(pcm);
        }
    }
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
        1024,
        1,
    )
    .context("failed to construct resampler")?;
    Ok(Some(ResamplerState::new(resampler, 1024)))
}

struct SelectedInput {
    device: cpal::Device,
    name: String,
    config: cpal::SupportedStreamConfig,
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
                return Ok(SelectedInput {
                    device,
                    name,
                    config,
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
        let config = match device.default_input_config() {
            Ok(config) => config,
            Err(_) => continue,
        };
        let selected = SelectedInput {
            device,
            name: name.clone(),
            config,
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

fn selected_mic_info(host: &cpal::Host) -> Result<MicInfo> {
    let selected = select_input_device(host)?;
    Ok(mic_snapshot_from_selected(&selected).0)
}

fn selected_mic_identity(host: &cpal::Host) -> Result<MicIdentity> {
    let selected = select_input_device(host)?;
    // This runs from the idle device poll. Keep it raw and CPAL-only: enriched
    // Linux info shells out to pactl and must stay on startup/reopen paths.
    Ok(raw_mic_identity_from_selected(&selected))
}

fn mic_snapshot_from_selected(selected: &SelectedInput) -> (MicInfo, MicIdentity) {
    let raw_identity = raw_mic_identity_from_selected(selected);
    let mut info = mic_info_from_identity(&raw_identity);
    enhance_mic_info(&mut info, selected.is_default);
    (info, raw_identity)
}

fn raw_mic_identity_from_selected(selected: &SelectedInput) -> MicIdentity {
    mic_identity_from_config(&selected.name, &selected.config)
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
    chunk_size: usize,
}

impl ResamplerState {
    fn new(resampler: SincFixedIn<f32>, chunk_size: usize) -> Self {
        Self {
            resampler,
            scratch: Vec::with_capacity(chunk_size * 4),
            chunk_size,
        }
    }

    fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.scratch.extend_from_slice(input);
        while self.scratch.len() >= self.chunk_size {
            let drained: Vec<f32> = self.scratch.drain(..self.chunk_size).collect();
            self.process_chunk(drained, out);
        }
    }

    fn flush_recording(&mut self, out: &mut Vec<f32>) {
        if !self.scratch.is_empty() {
            let mut padded = Vec::with_capacity(self.chunk_size);
            padded.extend_from_slice(&self.scratch);
            padded.resize(self.chunk_size, 0.0);
            self.scratch.clear();
            self.process_chunk(padded, out);
        }
        self.resampler.reset();
    }

    fn reset_recording(&mut self) {
        self.scratch.clear();
        self.resampler.reset();
    }

    fn process_chunk(&mut self, chunk: Vec<f32>, out: &mut Vec<f32>) {
        let input_frames = vec![chunk];
        match self.resampler.process(&input_frames, None) {
            Ok(out_frames) => {
                if let Some(ch0) = out_frames.into_iter().next() {
                    out.extend_from_slice(&ch0);
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
    wake: Sender<()>,
) -> Result<Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32> + 'static,
    f32: cpal::FromSample<T>,
{
    let mut mono_scratch: Vec<f32> = Vec::with_capacity(8192);
    let err_state = Arc::clone(&stream_error);
    let err_fn = move |err: cpal::StreamError| {
        *err_state.lock() = Some(err.to_string());
    };

    let stream = device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                mono_scratch.clear();
                if channels == 1 {
                    mono_scratch.reserve(data.len());
                    for &s in data {
                        mono_scratch.push(cpal::Sample::from_sample(s));
                    }
                } else {
                    let frames = data.len() / channels;
                    mono_scratch.reserve(frames);
                    for f in 0..frames {
                        let mut sum = 0.0f32;
                        for c in 0..channels {
                            let s: f32 = cpal::Sample::from_sample(data[f * channels + c]);
                            sum += s;
                        }
                        mono_scratch.push(sum / channels as f32);
                    }
                }

                let _ = producer.push_slice(&mono_scratch);
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
