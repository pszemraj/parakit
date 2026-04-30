//! Microphone capture into a shared f32 buffer at 16 kHz mono.
//!
//! The CPAL stream is owned by a dedicated manager thread. That keeps stream
//! creation and teardown on one thread, lets the daemon reopen the stream when
//! the OS default input changes, and avoids crashing when a USB or Bluetooth
//! microphone disappears.

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use crossbeam_channel::bounded;
use parking_lot::Mutex;
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::logging::Logger;

pub use parakit::constants::TARGET_RATE;

/// Reusable capacity for ordinary dictation bursts.
const RECORDING_CAPACITY: usize = TARGET_RATE as usize * 90;
/// Hard cap for one held recording to prevent unbounded memory growth.
const MAX_RECORDING_SAMPLES: usize = TARGET_RATE as usize * 60 * 5;
const DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Send-Sync handle that worker threads use to control and read the buffer.
#[derive(Clone)]
pub struct AudioHandle {
    buffer: Arc<Mutex<Vec<f32>>>,
    recording: Arc<AtomicBool>,
}

impl AudioHandle {
    /// Clear the current buffer and begin appending microphone samples.
    pub fn start_recording(&self) {
        let mut buf = self.buffer.lock();
        let capacity = buf.capacity();
        if capacity < RECORDING_CAPACITY {
            buf.reserve_exact(RECORDING_CAPACITY - capacity);
        }
        buf.clear();
        self.recording.store(true, Ordering::Release);
    }

    /// Stop recording and take ownership of the buffered samples.
    ///
    /// # Returns
    ///
    /// The captured mono PCM samples at [`TARGET_RATE`].
    pub fn stop_recording(&self) -> Vec<f32> {
        self.recording.store(false, Ordering::Release);
        std::mem::replace(
            &mut *self.buffer.lock(),
            Vec::with_capacity(RECORDING_CAPACITY),
        )
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
}

/// Live audio capture manager.
pub struct AudioCapture {
    /// Cloneable buffer control handle used by worker and hotkey threads.
    pub handle: AudioHandle,
    current: Arc<Mutex<Option<MicInfo>>>,
    alive: Arc<AtomicBool>,
    _thread: JoinHandle<()>,
}

impl AudioCapture {
    /// Open the best available input device and start the manager thread.
    ///
    /// # Returns
    ///
    /// A live capture manager plus a cloneable [`AudioHandle`].
    ///
    /// # Errors
    ///
    /// Returns an error if no usable input device can be opened.
    pub fn open(log: Arc<Logger>) -> Result<Self> {
        let buffer = Arc::new(Mutex::new(Vec::with_capacity(RECORDING_CAPACITY)));
        let recording = Arc::new(AtomicBool::new(false));
        let current = Arc::new(Mutex::new(None));
        let alive = Arc::new(AtomicBool::new(true));
        let stream_error = Arc::new(Mutex::new(None));

        let handle = AudioHandle {
            buffer: Arc::clone(&buffer),
            recording: Arc::clone(&recording),
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
                    buffer,
                    recording,
                    current: thread_current,
                    alive: thread_alive,
                    stream_error: thread_error,
                    log: thread_log,
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
    buffer: Arc<Mutex<Vec<f32>>>,
    recording: Arc<AtomicBool>,
    current: Arc<Mutex<Option<MicInfo>>>,
    alive: Arc<AtomicBool>,
    stream_error: Arc<Mutex<Option<String>>>,
    log: Arc<Logger>,
    ready: crossbeam_channel::Sender<Result<MicInfo>>,
}

fn audio_manager_loop(ctx: AudioManagerCtx) {
    let host = cpal::default_host();
    let mut live = match open_live_stream(
        &host,
        Arc::clone(&ctx.buffer),
        Arc::clone(&ctx.recording),
        Arc::clone(&ctx.stream_error),
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
        thread::sleep(DEVICE_POLL_INTERVAL);

        if let Some(err) = ctx.stream_error.lock().take() {
            ctx.log
                .warn(format!("microphone stream failed ({err}); reopening"));
            live = reopen_until_success(&host, &ctx);
            continue;
        }

        if ctx.recording.load(Ordering::Relaxed) {
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
                live = reopen_until_success(&host, &ctx);
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

fn reopen_until_success(host: &cpal::Host, ctx: &AudioManagerCtx) -> LiveStream {
    loop {
        match open_live_stream(
            host,
            Arc::clone(&ctx.buffer),
            Arc::clone(&ctx.recording),
            Arc::clone(&ctx.stream_error),
        ) {
            Ok(live) => {
                *ctx.current.lock() = Some(live.info.clone());
                return live;
            }
            Err(err) => {
                *ctx.current.lock() = None;
                ctx.log
                    .warn(format!("no usable microphone yet ({err:#}); retrying"));
                thread::sleep(DEVICE_POLL_INTERVAL);
            }
        }
    }
}

struct LiveStream {
    info: MicInfo,
    identity: MicIdentity,
    _stream: Stream,
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
    buffer: Arc<Mutex<Vec<f32>>>,
    recording: Arc<AtomicBool>,
    stream_error: Arc<Mutex<Option<String>>>,
) -> Result<LiveStream> {
    let selected = select_input_device(host)?;
    let identity = mic_identity_from_config(&selected.name, &selected.config);
    let mut info = mic_info_from_identity(&identity);
    enhance_mic_info(&mut info, selected.is_default);

    let hw_rate = selected.config.sample_rate().0;
    let channels = selected.config.channels() as usize;
    let stream_config: StreamConfig = selected.config.config();
    let resampler_state = make_resampler(hw_rate)?;

    let stream = match selected.config.sample_format() {
        SampleFormat::I8 => build_stream::<i8>(
            &selected.device,
            &stream_config,
            channels,
            buffer,
            recording,
            resampler_state,
            stream_error,
        )?,
        SampleFormat::I16 => build_stream::<i16>(
            &selected.device,
            &stream_config,
            channels,
            buffer,
            recording,
            resampler_state,
            stream_error,
        )?,
        SampleFormat::I32 => build_stream::<i32>(
            &selected.device,
            &stream_config,
            channels,
            buffer,
            recording,
            resampler_state,
            stream_error,
        )?,
        SampleFormat::U8 => build_stream::<u8>(
            &selected.device,
            &stream_config,
            channels,
            buffer,
            recording,
            resampler_state,
            stream_error,
        )?,
        SampleFormat::U16 => build_stream::<u16>(
            &selected.device,
            &stream_config,
            channels,
            buffer,
            recording,
            resampler_state,
            stream_error,
        )?,
        SampleFormat::U32 => build_stream::<u32>(
            &selected.device,
            &stream_config,
            channels,
            buffer,
            recording,
            resampler_state,
            stream_error,
        )?,
        SampleFormat::F32 => build_stream::<f32>(
            &selected.device,
            &stream_config,
            channels,
            buffer,
            recording,
            resampler_state,
            stream_error,
        )?,
        SampleFormat::F64 => build_stream::<f64>(
            &selected.device,
            &stream_config,
            channels,
            buffer,
            recording,
            resampler_state,
            stream_error,
        )?,
        other => return Err(anyhow!("unsupported sample format: {:?}", other)),
    };

    stream.play().context("stream.play() failed")?;
    Ok(LiveStream {
        info,
        identity,
        _stream: stream,
    })
}

fn make_resampler(hw_rate: u32) -> Result<Option<ResamplerState>> {
    if hw_rate == TARGET_RATE {
        return Ok(None);
    }

    let params = SincInterpolationParameters {
        sinc_len: 128,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    let resampler =
        SincFixedIn::<f32>::new(TARGET_RATE as f64 / hw_rate as f64, 2.0, params, 1024, 1)
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
    let identity = mic_identity_from_config(&selected.name, &selected.config);
    let mut info = mic_info_from_identity(&identity);
    enhance_mic_info(&mut info, selected.is_default);
    Ok(info)
}

fn selected_mic_identity(host: &cpal::Host) -> Result<MicIdentity> {
    let selected = select_input_device(host)?;
    Ok(mic_identity_from_config(&selected.name, &selected.config))
}

fn mic_identity_from_config(name: &str, config: &cpal::SupportedStreamConfig) -> MicIdentity {
    MicIdentity {
        name: name.to_string(),
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
    info.resampling = info.input_rate != TARGET_RATE;
}

#[cfg(not(target_os = "linux"))]
fn enhance_mic_info(_info: &mut MicInfo, _is_default: bool) {}

#[cfg(target_os = "linux")]
#[derive(Debug, Default, Eq, PartialEq)]
struct PactlSourceInfo {
    name: String,
    description: Option<String>,
    rate: Option<u32>,
    channels: Option<u16>,
    sample_format: Option<String>,
}

#[cfg(target_os = "linux")]
fn pactl_default_source_info() -> Option<PactlSourceInfo> {
    let default = std::process::Command::new("pactl")
        .args(["get-default-source"])
        .output()
        .ok()?;
    if !default.status.success() {
        return None;
    }
    let default_name = String::from_utf8_lossy(&default.stdout).trim().to_string();
    if default_name.is_empty() {
        return None;
    }

    let sources = std::process::Command::new("pactl")
        .args(["list", "sources"])
        .output()
        .ok()?;
    if !sources.status.success() {
        return None;
    }
    let sources = String::from_utf8_lossy(&sources.stdout);
    parse_pactl_sources(&sources)
        .into_iter()
        .find(|source| source.name == default_name)
}

#[cfg(target_os = "linux")]
fn parse_pactl_sources(text: &str) -> Vec<PactlSourceInfo> {
    let mut out = Vec::new();
    let mut current: Option<PactlSourceInfo> = None;

    for line in text.lines() {
        if line.starts_with("Source #") {
            if let Some(source) = current.take() {
                out.push(source);
            }
            current = Some(PactlSourceInfo::default());
            continue;
        }

        let Some(source) = current.as_mut() else {
            continue;
        };
        let trimmed = line.trim_start();
        if let Some(name) = trimmed.strip_prefix("Name: ") {
            source.name = name.trim().to_string();
        } else if let Some(description) = trimmed.strip_prefix("Description: ") {
            source.description = Some(description.trim().to_string());
        } else if let Some(spec) = trimmed.strip_prefix("Sample Specification: ") {
            let (sample_format, channels, rate) = parse_sample_spec(spec.trim());
            source.sample_format = sample_format;
            source.channels = channels;
            source.rate = rate;
        }
    }

    if let Some(source) = current {
        out.push(source);
    }
    out
}

#[cfg(target_os = "linux")]
fn parse_sample_spec(spec: &str) -> (Option<String>, Option<u16>, Option<u32>) {
    let mut parts = spec.split_whitespace();
    let sample_format = parts.next().map(str::to_string);
    let channels = parts
        .next()
        .and_then(|part| part.strip_suffix("ch"))
        .and_then(|part| part.parse().ok());
    let rate = parts
        .next()
        .and_then(|part| part.strip_suffix("Hz"))
        .and_then(|part| part.parse().ok());
    (sample_format, channels, rate)
}

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

/// Per-callback state for the resampler.
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
            let input_frames = vec![drained];
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
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    buffer: Arc<Mutex<Vec<f32>>>,
    recording: Arc<AtomicBool>,
    mut resampler: Option<ResamplerState>,
    stream_error: Arc<Mutex<Option<String>>>,
) -> Result<Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32> + 'static,
    f32: cpal::FromSample<T>,
{
    let mut mono_scratch: Vec<f32> = Vec::with_capacity(8192);
    let mut resampled_scratch: Vec<f32> = Vec::with_capacity(8192);
    let err_state = Arc::clone(&stream_error);
    let err_fn = move |err: cpal::StreamError| {
        *err_state.lock() = Some(err.to_string());
    };

    let stream = device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                if !recording.load(Ordering::Relaxed) {
                    return;
                }

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

                let to_push: &[f32] = match &mut resampler {
                    Some(r) => {
                        resampled_scratch.clear();
                        r.process(&mono_scratch, &mut resampled_scratch);
                        &resampled_scratch
                    }
                    None => &mono_scratch,
                };

                if to_push.is_empty() {
                    return;
                }

                append_recording_samples(&buffer, to_push);
            },
            err_fn,
            None,
        )
        .context("failed to build input stream")?;

    Ok(stream)
}

fn append_recording_samples(buffer: &Mutex<Vec<f32>>, samples: &[f32]) {
    let mut buf = buffer.lock();
    if buf.len() >= MAX_RECORDING_SAMPLES {
        return;
    }

    let remaining = MAX_RECORDING_SAMPLES - buf.len();
    let n = samples.len().min(remaining);
    buf.extend_from_slice(&samples[..n]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_input_names_are_filtered() {
        assert!(is_virtual_input_name(
            "Monitor of RODE NT-USB+ Analog Stereo"
        ));
        assert!(is_virtual_input_name("BlackHole 2ch"));
        assert!(is_virtual_input_name("PulseAudio Loopback"));
        assert!(!is_virtual_input_name("RODE NT-USB+ Mono"));
        assert!(!is_virtual_input_name("Bluetooth Headset"));
    }

    #[test]
    fn mic_summary_reports_input_and_model_rates() {
        let mic = MicInfo {
            name: "RODE NT-USB+ Mono".to_string(),
            input_rate: 48_000,
            channels: 1,
            sample_format: "F32".to_string(),
            resampling: true,
        };
        assert_eq!(
            mic.summary(),
            "RODE NT-USB+ Mono, 48000 Hz input -> 16000 Hz model, mono, F32"
        );
    }

    #[test]
    fn audio_handle_reuses_recording_capacity_after_stop() {
        let handle = AudioHandle {
            buffer: Arc::new(Mutex::new(Vec::new())),
            recording: Arc::new(AtomicBool::new(false)),
        };

        handle.start_recording();
        {
            let mut buffer = handle.buffer.lock();
            assert!(buffer.capacity() >= RECORDING_CAPACITY);
            buffer.extend_from_slice(&[0.25, -0.25]);
        }

        let pcm = handle.stop_recording();
        assert_eq!(pcm, vec![0.25, -0.25]);
        assert!(handle.buffer.lock().capacity() >= RECORDING_CAPACITY);
    }

    #[test]
    fn append_recording_samples_honors_hard_cap() {
        let buffer = Mutex::new(vec![0.0; MAX_RECORDING_SAMPLES - 1]);
        append_recording_samples(&buffer, &[1.0, 2.0, 3.0]);

        let buffer = buffer.lock();
        assert_eq!(buffer.len(), MAX_RECORDING_SAMPLES);
        assert_eq!(buffer[MAX_RECORDING_SAMPLES - 1], 1.0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pactl_source_parser_extracts_description_and_rate() {
        let sources = parse_pactl_sources(
            r#"Source #42
    Name: alsa_input.usb-RODE_NT-USB-00.mono-fallback
    Description: RODE NT-USB+ Mono
    Sample Specification: s24le 1ch 48000Hz
Source #43
    Name: alsa_output.pci-0000_00.monitor
    Description: Monitor of HDMI Audio
    Sample Specification: s32le 2ch 48000Hz
"#,
        );
        assert_eq!(sources.len(), 2);
        assert_eq!(
            sources[0],
            PactlSourceInfo {
                name: "alsa_input.usb-RODE_NT-USB-00.mono-fallback".to_string(),
                description: Some("RODE NT-USB+ Mono".to_string()),
                rate: Some(48_000),
                channels: Some(1),
                sample_format: Some("s24le".to_string()),
            }
        );
    }
}
