//! CPAL stream lifecycle: the audio manager thread, live stream open and recovery, and the input callback.

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use crossbeam_channel::{bounded, Sender};
use parking_lot::Mutex;
use ringbuf::{
    traits::{Producer, Split},
    HeapProd, HeapRb,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::device::{
    mic_info_from_identity, mic_snapshot_from_selected, select_input_device, selected_mic_identity,
    MicIdentity, MicInfo,
};
use super::drain::{make_resampler, spawn_audio_drain, AudioDrain, CapturePipeline, DrainControl};
use super::monitor::{MicChanges, MicProbe};
use super::{recv_audio_control_ack, AudioControl, AudioManagerCtx, CaptureState, TARGET_RATE};

const AUDIO_RING_SECONDS: usize = 6;
const AUDIO_RING_MIN_CAPACITY: usize = TARGET_RATE as usize * AUDIO_RING_SECONDS;
const DEFAULT_CALLBACK_SCRATCH_FRAMES: usize = 8192;
const MANAGER_TICK: Duration = Duration::from_secs(1);
const DEVICE_POLL_INTERVAL: Duration = Duration::from_secs(5);
const DEVICE_REOPEN_INTERVAL: Duration = Duration::from_secs(10);
const MICROPHONE_NOTICE_INTERVAL: Duration = Duration::from_secs(30);
const DEVICE_RETRY_MAX_INTERVAL: Duration = Duration::from_secs(10);

/// Own the live stream on the manager thread, serving start and stop commands and reopening on failure or input change.
pub(super) fn audio_manager_loop(ctx: AudioManagerCtx) {
    let host = cpal::default_host();
    let idle_policy = idle_stream_policy();
    let mut probe = match MicProbe::spawn(|| selected_mic_identity(&cpal::default_host())) {
        Ok(probe) => probe,
        Err(err) => {
            let _ = ctx.ready.send(Err(err));
            return;
        }
    };
    let live = match open_live_stream(
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

    let mut changes = MicChanges::default();
    let mut generation = 0_u64;
    let mut next_probe = Instant::now() + DEVICE_POLL_INTERVAL;
    let mut next_reopen = Instant::now();
    let mut retry_delay = MANAGER_TICK;
    let mut attempts = 0_u32;
    let mut last_notice = None;
    let mut report_recovery = false;
    let mut healthy_since = Instant::now();
    let mut reopen_identity = live.identity.clone();
    let mut live = Some(live);

    while ctx.alive.load(Ordering::SeqCst) {
        // Release the callback's mutex before dropping or opening a CPAL stream.
        let stream_error = ctx.stream_error.lock().take();
        if let Some(err) = stream_error {
            ctx.log
                .warn(format!("microphone stream failed ({err}); reopening"));
            if last_notice.is_none_or(|at: Instant| at.elapsed() >= MICROPHONE_NOTICE_INTERVAL) {
                ctx.notifier.microphone_unavailable(&err);
                last_notice = Some(Instant::now());
                report_recovery = true;
            }
            if let Some(previous) = live.take() {
                reopen_identity = previous.identity.clone();
                drop(previous);
                ctx.stream_error.lock().take();
                generation = generation.wrapping_add(1);
            }
            *ctx.current.lock() = None;
            changes.reset();
        }

        if live.is_none() && Instant::now() >= next_reopen {
            // Errors from the old stream cannot describe the replacement.
            ctx.stream_error.lock().take();
            match open_live_stream(
                &host,
                Arc::clone(&ctx.state),
                Arc::clone(&ctx.session_epoch),
                Arc::clone(&ctx.stream_error),
                idle_policy.paused_when_idle && ctx.session_epoch.load(Ordering::Acquire) == 0,
            ) {
                Ok(mut opened) => {
                    opened.identity.retain_source(&reopen_identity);
                    *ctx.current.lock() = Some(opened.info.clone());
                    ctx.log.mic_changed(&opened.info);
                    live = Some(opened);
                    generation = generation.wrapping_add(1);
                    healthy_since = Instant::now();
                    next_reopen = healthy_since + DEVICE_REOPEN_INTERVAL;
                    next_probe = healthy_since + DEVICE_POLL_INTERVAL;
                    retry_delay = MANAGER_TICK;
                    attempts = 0;
                }
                Err(err) => {
                    attempts = attempts.saturating_add(1);
                    if attempts == 1 || attempts.is_multiple_of(30) {
                        ctx.log
                            .warn(format!("no usable microphone yet ({err:#}); retrying"));
                    } else {
                        ctx.log
                            .verbose(format!("microphone still unavailable ({err:#})"));
                    }
                    next_reopen = Instant::now() + retry_delay;
                    retry_delay = (retry_delay * 2).min(DEVICE_RETRY_MAX_INTERVAL);
                }
            }
        }

        if let Some(current) = &live {
            if report_recovery && healthy_since.elapsed() >= DEVICE_REOPEN_INTERVAL {
                ctx.notifier.microphone_recovered(&current.info);
                last_notice = Some(Instant::now());
                report_recovery = false;
            }
            let dropped_samples = current.dropped_samples.swap(0, Ordering::AcqRel);
            if dropped_samples != 0 {
                ctx.log.warn(format!(
                    "microphone ring overflow dropped {dropped_samples} sample(s)"
                ));
            }
            if ctx.session_epoch.load(Ordering::Relaxed) == 0 && Instant::now() >= next_probe {
                probe.request(generation);
            }
        }

        let wait = if live.is_none() {
            next_reopen
                .saturating_duration_since(Instant::now())
                .min(MANAGER_TICK)
        } else {
            MANAGER_TICK
        };
        crossbeam_channel::select! {
            recv(ctx.control_rx) -> message => {
                let Ok(control) = message else { break; };
                // A queued callback error makes Start fail promptly, even before
                // the next loop takes that error and tears the stream down.
                if let Some(current) = live.as_mut().filter(|_| ctx.stream_error.lock().is_none()) {
                    handle_manager_control(control, current, &ctx, idle_policy);
                } else {
                    handle_unavailable_control(control, &ctx);
                }
                changes.reset();
            }
            recv(probe.results) -> result => {
                let Ok((observed_generation, result)) = result else { break; };
                probe.pending = false;
                next_probe = Instant::now() + DEVICE_POLL_INTERVAL;
                if observed_generation != generation || ctx.session_epoch.load(Ordering::Relaxed) != 0 {
                    changes.reset();
                    continue;
                }
                let Some(current) = live.as_ref() else { continue; };
                match result {
                    Ok(next) => {
                        if let Some(confirmed) = changes.observe(&current.identity, next) {
                            if Instant::now() >= next_reopen {
                                ctx.log.verbose(format!(
                                    "parakit: selected input changed from {} to {}",
                                    current.info.summary(), mic_info_from_identity(&confirmed).summary()
                                ));
                                reopen_identity = confirmed;
                                drop(live.take());
                                ctx.stream_error.lock().take();
                                *ctx.current.lock() = None;
                                generation = generation.wrapping_add(1);
                            } else {
                                next_probe = next_reopen;
                            }
                        } else if changes.is_pending() {
                            next_probe = Instant::now() + MANAGER_TICK;
                        }
                    }
                    Err(err) => {
                        changes.reset();
                        ctx.log.verbose(format!("parakit: input device scan failed: {err:#}"));
                    }
                }
            }
            default(wait) => {}
        }
    }
}

/// Reject new captures while retrying, but preserve PCM from an interrupted recording.
fn handle_unavailable_control(control: AudioControl, ctx: &AudioManagerCtx) {
    match control {
        AudioControl::Start { ack, .. } => {
            let _ = ack.send(Err(anyhow!("microphone is unavailable; retrying")));
        }
        AudioControl::Stop { ack } => {
            ctx.session_epoch.store(0, Ordering::Release);
            let pcm = ctx.state.lock().take_recording();
            let _ = ack.send(Ok(pcm));
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
    recv_audio_control_ack(ack_rx, "audio drain", "Start")
}

fn stop_live_recording(live: &mut LiveStream) -> Result<Vec<f32>> {
    let (ack_tx, ack_rx) = bounded(1);
    live.drain_control
        .send(DrainControl::Stop { ack: ack_tx })
        .context("audio drain is not available")?;
    recv_audio_control_ack(ack_rx, "audio drain", "Stop")
}

fn pause_live_stream(live: &mut LiveStream, idle_policy: IdleStreamPolicy) -> Result<()> {
    if !idle_policy.paused_when_idle || live.paused {
        return Ok(());
    }
    live.stream.pause().context("stream.pause() failed")?;
    live.paused = true;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon::logging::{LogLevel, Logger};
    use crate::daemon::notifications::Notifier;

    #[test]
    fn unavailable_microphone_rejects_start_and_returns_retained_audio_on_stop() {
        let state = Arc::new(Mutex::new(CaptureState::new()));
        let samples = vec![0.25, -0.5, 0.75];
        state.lock().buffer = samples.clone();
        let epoch = Arc::new(AtomicU64::new(7));
        let log = Arc::new(Logger::new(LogLevel::Quiet));
        let ctx = AudioManagerCtx {
            state,
            session_epoch: Arc::clone(&epoch),
            current: Arc::new(Mutex::new(None)),
            alive: Arc::new(AtomicBool::new(true)),
            stream_error: Arc::new(Mutex::new(None)),
            control_rx: bounded(1).1,
            log: Arc::clone(&log),
            notifier: Notifier::silent(log),
            ready: bounded(1).0,
        };
        let (ack, reply) = bounded(1);
        handle_unavailable_control(AudioControl::Start { epoch: 8, ack }, &ctx);
        assert!(reply
            .try_recv()
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("unavailable"));
        assert_eq!(epoch.load(Ordering::Acquire), 7);
        let (ack, reply) = bounded(1);
        handle_unavailable_control(AudioControl::Stop { ack }, &ctx);
        assert_eq!(reply.try_recv().unwrap().unwrap(), samples);
        assert_eq!(epoch.load(Ordering::Acquire), 0);
        assert!(ctx.state.lock().buffer.is_empty());
    }
}
