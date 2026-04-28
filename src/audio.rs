//! Microphone capture into a shared f32 buffer at 16 kHz mono.
//!
//! Strategy:
//!   1. Open the default input device. Try to request 16 kHz mono f32 directly.
//!   2. If the device doesn't support 16 kHz, open at its preferred rate and
//!      resample on-the-fly via `rubato` (high-quality sinc).
//!   3. Multi-channel input is mixed to mono by averaging.
//!   4. While `recording` is true, append samples to `buffer`.
//!
//! API split:
//!   - [`AudioCapture`] owns the live `cpal::Stream` and is `!Send` (cpal
//!     streams must stay on the thread that built them on some platforms).
//!     Lives on the main thread.
//!   - [`AudioHandle`] is the `Send + Sync` view that worker threads can
//!     hold to start/stop/snapshot the buffer.

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use parking_lot::Mutex;
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub use parakit::constants::TARGET_RATE;

/// Reserves enough capacity for ~10 minutes of recording at 16 kHz.
const PREALLOC_CAPACITY: usize = TARGET_RATE as usize * 60 * 10;

/// Send-Sync handle that worker threads use to control / read the buffer.
#[derive(Clone)]
pub struct AudioHandle {
    buffer: Arc<Mutex<Vec<f32>>>,
    recording: Arc<AtomicBool>,
}

impl AudioHandle {
    pub fn start_recording(&self) {
        self.buffer.lock().clear();
        self.recording.store(true, Ordering::SeqCst);
    }

    pub fn stop_recording(&self) -> Vec<f32> {
        self.recording.store(false, Ordering::SeqCst);
        std::mem::take(&mut *self.buffer.lock())
    }

    pub fn snapshot_from(&self, from: usize) -> Vec<f32> {
        let buf = self.buffer.lock();
        if from >= buf.len() {
            Vec::new()
        } else {
            buf[from..].to_vec()
        }
    }

    pub fn len(&self) -> usize {
        self.buffer.lock().len()
    }
}

/// Holds the live audio stream. Drop to stop the stream cleanly.
/// Not `Send` because of `cpal::Stream`.
pub struct AudioCapture {
    pub handle: AudioHandle,
    pub hw_rate: u32,
    pub resampling: bool,
    _stream: Stream,
}

impl AudioCapture {
    pub fn open() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow!("no default input device"))?;

        let supported_configs = device
            .supported_input_configs()
            .context("failed to list input configs")?;

        let mut chosen: Option<cpal::SupportedStreamConfig> = None;
        for cfg in supported_configs {
            if cfg.sample_format() == SampleFormat::F32
                && cfg.min_sample_rate().0 <= TARGET_RATE
                && cfg.max_sample_rate().0 >= TARGET_RATE
            {
                chosen = Some(cfg.with_sample_rate(cpal::SampleRate(TARGET_RATE)));
                break;
            }
        }
        let chosen = match chosen {
            Some(c) => c,
            None => device
                .default_input_config()
                .context("no usable input config")?,
        };

        let hw_rate = chosen.sample_rate().0;
        let channels = chosen.channels() as usize;
        let resampling = hw_rate != TARGET_RATE;

        let buffer = Arc::new(Mutex::new(Vec::with_capacity(PREALLOC_CAPACITY)));
        let recording = Arc::new(AtomicBool::new(false));

        let buffer_cb = Arc::clone(&buffer);
        let recording_cb = Arc::clone(&recording);

        let stream_config: StreamConfig = chosen.config();

        let resampler_state = if resampling {
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
            Some(ResamplerState::new(resampler, 1024))
        } else {
            None
        };

        let stream = match chosen.sample_format() {
            SampleFormat::F32 => build_stream::<f32>(
                &device,
                &stream_config,
                channels,
                buffer_cb,
                recording_cb,
                resampler_state,
            )?,
            SampleFormat::I16 => build_stream::<i16>(
                &device,
                &stream_config,
                channels,
                buffer_cb,
                recording_cb,
                resampler_state,
            )?,
            SampleFormat::U16 => build_stream::<u16>(
                &device,
                &stream_config,
                channels,
                buffer_cb,
                recording_cb,
                resampler_state,
            )?,
            other => return Err(anyhow!("unsupported sample format: {:?}", other)),
        };

        stream.play().context("stream.play() failed")?;

        Ok(Self {
            handle: AudioHandle { buffer, recording },
            hw_rate,
            resampling,
            _stream: stream,
        })
    }
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
) -> Result<Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32> + 'static,
    f32: cpal::FromSample<T>,
{
    let mut mono_scratch: Vec<f32> = Vec::with_capacity(8192);
    let mut resampled_scratch: Vec<f32> = Vec::with_capacity(8192);

    let err_fn = |err| eprintln!("parakit: cpal stream error: {err}");

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

                let mut buf = buffer.lock();
                buf.extend_from_slice(to_push);
            },
            err_fn,
            None,
        )
        .context("failed to build input stream")?;

    Ok(stream)
}
