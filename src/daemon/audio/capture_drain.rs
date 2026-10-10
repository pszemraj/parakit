//! Sample-ring drain thread, recording pipeline, and resampling.

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender};
use parakit::audio_file::{process_resample_chunk, resampler_params, RESAMPLE_CHUNK_SIZE};
use parking_lot::Mutex;
use ringbuf::{traits::Consumer, HeapCons};
use rubato::{Resampler, SincFixedIn};
use std::io::Write as _;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use super::{append_processed_samples, CaptureState, TARGET_RATE};

/// Scratch capacity for the drain loop's per-iteration input/resample buffers.
/// Sized independently of [`DEFAULT_CALLBACK_SCRATCH_FRAMES`]; the two happen
/// to share a value but are separate sizing decisions.
///
/// [`DEFAULT_CALLBACK_SCRATCH_FRAMES`]: super::DEFAULT_CALLBACK_SCRATCH_FRAMES
pub(super) const DRAIN_SCRATCH_FRAMES: usize = 8192;

/// Commands the drain thread accepts to begin or end a recording.
pub(super) enum DrainControl {
    Start {
        epoch: u64,
        include_pre_roll: bool,
        ack: Sender<()>,
    },
    Stop {
        ack: Sender<Vec<f32>>,
    },
}

/// Owner of the drain thread; dropping it stops and joins the thread.
pub(super) struct AudioDrain {
    pub(super) alive: Arc<AtomicBool>,
    pub(super) wake: Sender<()>,
    pub(super) thread: Option<JoinHandle<()>>,
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

/// Spawn the thread that drains the sample ring into the recording state.
///
/// # Returns
///
/// The join handle of the drain thread.
///
/// # Errors
///
/// Returns an error if the thread cannot be spawned.
///
/// # Arguments
///
/// * `consumer` - Consumer half of the sample ring.
/// * `wake_rx` - Wake signal sent by the input callback.
/// * `control_rx` - Start and stop commands for the drain.
/// * `state` - Shared recording buffer and pre-roll.
/// * `session_epoch` - Epoch of the active recording session, 0 when idle.
/// * `pipeline` - Resampling stage applied to drained samples.
/// * `alive` - Cleared to stop the drain thread.
pub(super) fn spawn_audio_drain(
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
    let mut input = vec![0.0_f32; DRAIN_SCRATCH_FRAMES];
    let mut resampled = Vec::with_capacity(DRAIN_SCRATCH_FRAMES);
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

/// Apply one start or stop command to the drain state.
///
/// # Arguments
///
/// * `control` - Command to apply.
/// * `consumer` - Consumer half of the sample ring.
/// * `state` - Shared recording buffer and pre-roll.
/// * `session_epoch` - Epoch of the active recording session, 0 when idle.
/// * `pipeline` - Resampling stage applied to drained samples.
/// * `input` - Scratch buffer for ring reads.
/// * `resampled` - Scratch buffer for resampled output.
pub(super) fn handle_audio_control(
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
            if ack.send(()).is_err() {
                // The manager timed out waiting for this queued Start. Do not
                // leave a capture active after its caller abandoned it.
                session_epoch.store(0, Ordering::Release);
                let _ = state.lock().take_recording();
                pipeline.reset_recording();
            }
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

/// Build the resampler for a hardware rate, or `None` when it already matches the model rate.
///
/// # Returns
///
/// The per-stream resampler state, or `None` when no resampling is needed.
///
/// # Errors
///
/// Returns an error if the resampler cannot be constructed.
///
/// # Panics
///
/// Does not panic.
pub(super) fn make_resampler(hw_rate: u32) -> Result<Option<ResamplerState>> {
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

/// Optional resampling stage between the sample ring and the recording buffer.
#[derive(Default)]
pub(super) struct CapturePipeline {
    pub(super) resampler: Option<ResamplerState>,
}

impl CapturePipeline {
    /// Discard partial resampler state at a recording boundary.
    pub(super) fn reset_recording(&mut self) {
        if let Some(resampler) = &mut self.resampler {
            resampler.reset_recording();
        }
    }

    /// Resample `input` into `out`, or pass it through when no resampler is active.
    ///
    /// # Returns
    ///
    /// The processed samples, borrowed from `input` or `out`.
    ///
    /// # Arguments
    ///
    /// * `input` - Mono samples drained from the ring.
    /// * `out` - Output buffer used when resampling.
    pub(super) fn process<'a>(&mut self, input: &'a [f32], out: &'a mut Vec<f32>) -> &'a [f32] {
        match &mut self.resampler {
            Some(resampler) => {
                out.clear();
                resampler.process(input, out);
                out
            }
            None => input,
        }
    }

    /// Flush the resampler tail of the finished recording into `out`.
    pub(super) fn finish_recording(&mut self, out: &mut Vec<f32>) {
        if let Some(resampler) = &mut self.resampler {
            resampler.flush_recording(out);
        }
    }
}

/// Per-stream state for resampling one recording at a time.
pub(super) struct ResamplerState {
    resampler: SincFixedIn<f32>,
    pub(super) scratch: Vec<f32>,
    pub(super) input_buf: Vec<Vec<f32>>,
    pub(super) output_buf: Vec<Vec<f32>>,
    pub(super) chunk_size: usize,
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
        if let Err(e) = process_resample_chunk(
            &mut self.resampler,
            &self.input_buf,
            &mut self.output_buf,
            out,
        ) {
            let _ = writeln!(
                std::io::stderr(),
                "parakit: resampler error (dropped chunk): {e}"
            );
        }
    }
}
