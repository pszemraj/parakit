//! CPAL stream lifecycle: the audio manager thread, live stream open and recovery, and the input callback.

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::Mutex;
use ringbuf::{
    traits::{Producer, Split},
    HeapProd, HeapRb,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use super::device::{
    mic_info_from_identity, mic_snapshot_from_selected, select_input_device, selected_mic_identity,
    MicIdentity, MicInfo,
};
use super::drain::{make_resampler, spawn_audio_drain, AudioDrain, CapturePipeline, DrainControl};
use super::{
    audio_control_ack_error, AudioControl, AudioManagerCtx, CaptureState, AUDIO_CONTROL_TIMEOUT,
    TARGET_RATE,
};

const AUDIO_RING_SECONDS: usize = 6;
const AUDIO_RING_MIN_CAPACITY: usize = TARGET_RATE as usize * AUDIO_RING_SECONDS;
const DEFAULT_CALLBACK_SCRATCH_FRAMES: usize = 8192;
const DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(1);
const DEVICE_RETRY_MAX_INTERVAL: Duration = Duration::from_secs(10);

/// Own the live stream on the manager thread, serving start and stop commands and reopening on failure or input change.
pub(super) fn audio_manager_loop(ctx: AudioManagerCtx) {
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
        .map_err(|err| audio_control_ack_error("audio drain", label, err))
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

    macro_rules! build_typed_stream {
        ($sample:ty) => {
            build_stream::<$sample>(
                &selected.device,
                &stream_config,
                channels,
                producer,
                stream_error,
                Arc::clone(&dropped_samples),
                wake_tx.clone(),
            )?
        };
    }

    let stream = match selected.config.sample_format() {
        SampleFormat::I8 => build_typed_stream!(i8),
        SampleFormat::I16 => build_typed_stream!(i16),
        SampleFormat::I32 => build_typed_stream!(i32),
        SampleFormat::U8 => build_typed_stream!(u8),
        SampleFormat::U16 => build_typed_stream!(u16),
        SampleFormat::U32 => build_typed_stream!(u32),
        SampleFormat::F32 => build_typed_stream!(f32),
        SampleFormat::F64 => build_typed_stream!(f64),
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
