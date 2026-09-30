//! Transcription worker and paste safety boundary.

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use parakit::data_log::{CleaningLogFields, DataLogger};
use parakit::inference::Engine;
use parakit::rules::{Cleaner, RuleHit, CLEANER_VERSION};
use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::audio::TARGET_RATE;
use super::engine_runtime::EngineRecipe;
use super::inject::{FocusSnapshot, PasteMode};
use super::ipc::SharedState;
use super::logging::Logger;
use super::model_lifecycle::{ActivityGuard, ModelSlot};
use super::notifications::Notifier;
use super::sounds::Sounds;

mod insertion;
pub(crate) use insertion::{insert_text, FocusCheck, InsertOutcome};
use insertion::{insertion_result_remembers_transcript, log_insertion_outcome, prepared_injector};

/// Maximum number of worker events that may queue while ASR or paste is busy.
pub(crate) const WORKER_QUEUE_CAPACITY: usize = 2;

const SILENCE_PEAK_THRESHOLD: f32 = 0.001;
const SILENCE_RMS_THRESHOLD: f32 = 0.0005;

/// Events consumed by the transcription worker.
pub(crate) enum WorkerEvent {
    /// Recording began at this instant.
    Started,
    /// Recording began but failed before PCM could be handed to the worker.
    Failed {
        /// User-facing failure message without the standard log prefix.
        message: String,
        /// Activity retained through delivery of the terminal failure.
        activity: Option<ActivityGuard>,
    },
    /// Recording ended and the captured PCM moved out of the audio buffer.
    Stopped {
        /// Monotonic timestamp captured when recording started.
        started_at: Instant,
        /// Monotonic timestamp captured when recording stopped.
        stopped_at: Instant,
        /// Owned 16 kHz mono PCM for this utterance.
        pcm: Vec<f32>,
        /// Focus target captured before audio recording began.
        focus_at_start: Option<Box<FocusSnapshot>>,
        /// Recording admission retained until processing and insertion finish.
        activity: Option<ActivityGuard>,
        /// Optional completion acknowledgement for the headless validation flow.
        completion: Option<Sender<Result<(), String>>>,
    },
}

/// Dependencies owned by the transcription worker thread.
pub(crate) struct WorkerCtx {
    /// Open transcription engine.
    pub(crate) engine: Engine,
    /// Startup parameters reused when the model has been offloaded.
    pub(crate) recipe: EngineRecipe,
    /// Effective user setting, also exposed in status.
    pub(crate) model_idle_minutes: u64,
    /// Optional transcript cleaner.
    pub(crate) cleaner: Option<Arc<Cleaner>>,
    /// Optional transcription metadata logger.
    pub(crate) data_log: Option<Arc<DataLogger>>,
    /// Audio cue player.
    pub(crate) sounds: Sounds,
    /// Process logger.
    pub(crate) log: Arc<Logger>,
    /// Desktop notification helper.
    pub(crate) notifier: Notifier,
    /// Shared state exposed through local IPC.
    pub(crate) state: Arc<SharedState>,
    /// Paste chord mode.
    pub(crate) paste_mode: PasteMode,
    /// Leave transcript text on the clipboard after paste/fallback instead of
    /// restoring previous supported clipboard contents.
    pub(crate) keep_transcript_clipboard: bool,
    /// Whether transcripts should be inserted after inference.
    pub(crate) insert_transcripts: bool,
    /// Worker event receiver.
    pub(crate) rx: Receiver<WorkerEvent>,
    /// Dropped after the worker's engine, so IPC can safely terminate native code.
    pub(crate) lifetime: super::worker_shutdown::WorkerLifetime,
}

struct TranscriptResult {
    raw: String,
    cleaned: String,
    /// Cleaning passes that changed the transcript, in application order.
    /// Empty when cleaning is disabled or when cleaning failed open.
    rules_fired: Vec<RuleHit>,
    /// Set when a cleaning pass failed at runtime and `cleaned` is the
    /// untransformed `raw` transcript rather than a cleaned one.
    cleaning_failure: Option<String>,
    infer_elapsed: Duration,
    clean_elapsed: Duration,
}

/// Start the transcription worker thread.
///
/// # Returns
///
/// A join handle for the worker thread.
pub(crate) fn spawn_worker(ctx: WorkerCtx) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || worker_loop(ctx))
}

fn worker_loop(ctx: WorkerCtx) {
    let WorkerCtx {
        engine,
        recipe,
        model_idle_minutes,
        cleaner,
        data_log,
        sounds,
        log,
        notifier,
        state,
        paste_mode,
        keep_transcript_clipboard,
        insert_transcripts,
        rx,
        lifetime: _lifetime,
    } = ctx;
    // Config/CLI validation has already checked the supported clock range.
    let timeout = super::model_lifecycle::idle_timeout(model_idle_minutes)
        .expect("validated model idle timeout");
    let mut model = ModelSlot::new(engine, model_idle_minutes);
    state.set_model_status(model.status());

    // Cleaner-derived telemetry is fixed for the worker's lifetime; only the
    // fired-rule list and any cleaning failure vary per utterance.
    let rules_active = cleaner.as_deref().map_or(0, Cleaner::active_rule_count);
    let cleaning_profile = cleaner
        .as_deref()
        .map_or("disabled", |cleaner| cleaner.profile().as_str());
    let ruleset_id = cleaner
        .as_deref()
        .map(|cleaner| cleaner.ruleset_id().to_string());
    let drops_trailing_period = cleaner
        .as_deref()
        .is_some_and(Cleaner::drops_trailing_period);
    let number_threshold = cleaner.as_deref().map(Cleaner::number_threshold);
    let mut injector = if insert_transcripts {
        match prepared_injector(paste_mode) {
            Ok(injector) => Some(injector),
            Err(err) => {
                log.error(&format!(
                    "insertion backend unavailable at worker startup: {err:#}"
                ));
                None
            }
        }
    } else {
        None
    };
    loop {
        if state.shutdown.requested() {
            break;
        }
        if model.offload(&state.activity, timeout) {
            state.set_model_status(model.status());
            log.verbose("parakit: model offloaded after idle timeout");
        }
        let deadline = if model.is_loaded() {
            state
                .activity
                .remaining(timeout)
                .map(crossbeam_channel::after)
        } else {
            None
        };
        let deadline = deadline.unwrap_or_else(crossbeam_channel::never);
        let ev = crossbeam_channel::select! {
            recv(rx) -> event => match event {
                Ok(event) => event,
                Err(_) => break,
            },
            recv(state.activity.changes()) -> _ => continue,
            recv(deadline) -> _ => continue,
        };
        if state.shutdown.requested() {
            break;
        }
        match ev {
            WorkerEvent::Started => {
                state.set_phase("recording");
                sounds.start();
                log.line("parakit: recording...");
                model.ensure_loaded(
                    || {
                        let (engine, device) = recipe.open(&log)?;
                        state.update_engine_info(engine.backend(), device);
                        Ok(engine)
                    },
                    |status| state.set_model_status(status),
                );
            }
            WorkerEvent::Failed { message, activity } => {
                let _activity = activity;
                log.error(&message);
                state.set_phase("idle");
                sounds.error();
            }
            WorkerEvent::Stopped {
                started_at,
                stopped_at,
                pcm,
                focus_at_start,
                activity,
                completion,
            } => {
                let _activity = activity;
                let mut completion = Completion::new(completion);
                let stop_started = Instant::now();
                let drain_elapsed = stop_started.saturating_duration_since(stopped_at);
                let secs = pcm.len() as f32 / TARGET_RATE as f32;
                let wall_secs = stopped_at.duration_since(started_at).as_secs_f32();
                if model.engine().is_ok() && capture_should_skip(&pcm) {
                    log.verbose(format!(
                        "parakit: skipped silent capture ({secs:.2}s audio, {wall_secs:.2}s wall)"
                    ));
                    log.line("parakit: no speech detected");
                    state.set_phase("idle");
                    sounds.success();
                    continue;
                }
                state.set_phase("transcribing");
                log.transcribing(secs, wall_secs);

                let transcription = model
                    .engine()
                    .and_then(|engine| transcribe_clean(engine, &pcm, cleaner.as_deref()));
                // Inference is the last PCM consumer. Release the potentially
                // maximum-length capture before modifier-release and paste
                // acknowledgement waits keep the worker occupied.
                drop(pcm);

                if state.shutdown.requested() {
                    break;
                }

                match transcription {
                    Ok(Some(transcript)) => {
                        completion.succeed();
                        // Count the dictation itself, once, regardless of how
                        // many insertion attempts or outcomes follow below.
                        state.record_dictation();
                        if let Some(failure) = transcript.cleaning_failure.as_deref() {
                            log.warn(format!(
                                "parakit: cleaning failed, inserting the raw transcript: {failure}"
                            ));
                        }
                        let record_id = data_log.as_ref().and_then(|data_log| {
                            data_log.log(
                                secs,
                                transcript.infer_elapsed,
                                &transcript.raw,
                                &transcript.cleaned,
                                CleaningLogFields {
                                    rules_active,
                                    cleaner_version: CLEANER_VERSION,
                                    profile: cleaning_profile,
                                    ruleset_id: ruleset_id.as_deref(),
                                    drops_trailing_period,
                                    number_threshold,
                                    rules_fired: &transcript.rules_fired,
                                    failure: transcript.cleaning_failure.as_deref(),
                                },
                            )
                        });
                        let transcript_chars = transcript.cleaned.chars().count();
                        log.transcript(
                            &transcript.raw,
                            &transcript.cleaned,
                            transcript.infer_elapsed,
                        );
                        if !insert_transcripts {
                            log.verbose("parakit: insertion skipped for PTT audio simulation");
                            log_insertion_outcome(
                                &data_log,
                                record_id,
                                "skipped",
                                focus_at_start.as_deref(),
                                transcript_chars,
                                None,
                                "not_applicable",
                                None,
                            );
                            state.remember_transcript(transcript.cleaned.clone());
                            state.set_phase("idle");
                            sounds.success();
                            continue;
                        }
                        let insert_started = Instant::now();
                        let cleaned = transcript.cleaned.clone();
                        let focus_verification = Cell::new("not_applicable");
                        let focus_check = FocusCheck {
                            snapshot: focus_at_start.as_deref(),
                            verification: &focus_verification,
                        };
                        let insert_result = state.with_insertion_lock(|| {
                            let result = insert_text(
                                &mut injector,
                                &cleaned,
                                paste_mode,
                                keep_transcript_clipboard,
                                focus_check,
                                (log.as_ref(), &notifier),
                            );
                            if insertion_result_remembers_transcript(&result) {
                                state.remember_transcript(cleaned);
                            }
                            result
                        });
                        match insert_result {
                            Ok(report) => {
                                let outcome = report.outcome;
                                log_insertion_outcome(
                                    &data_log,
                                    record_id,
                                    outcome.log_label(),
                                    focus_at_start.as_deref(),
                                    transcript_chars,
                                    None,
                                    focus_verification.get(),
                                    Some(&report),
                                );
                                let insert_elapsed = insert_started.elapsed();
                                let worker_elapsed = stop_started.elapsed();
                                let total_elapsed = drain_elapsed + worker_elapsed;
                                log.verbose(format!(
                                    "parakit: timings drain={}ms infer={}ms clean={}ms insert={}ms worker={}ms total={}ms",
                                    drain_elapsed.as_secs_f32() * 1000.0,
                                    transcript.infer_elapsed.as_secs_f32() * 1000.0,
                                    transcript.clean_elapsed.as_secs_f32() * 1000.0,
                                    insert_elapsed.as_secs_f32() * 1000.0,
                                    worker_elapsed.as_secs_f32() * 1000.0,
                                    total_elapsed.as_secs_f32() * 1000.0
                                ));
                                state.set_phase("idle");
                                if report.needs_alert() {
                                    sounds.error();
                                } else {
                                    sounds.success();
                                }
                            }
                            Err(e) => {
                                log_insertion_outcome(
                                    &data_log,
                                    record_id,
                                    "error",
                                    focus_at_start.as_deref(),
                                    transcript_chars,
                                    Some(format!("{e:#}")),
                                    focus_verification.get(),
                                    None,
                                );
                                log.error(&format!("paste failed: {e:#}"));
                                state.set_phase("idle");
                                sounds.error();
                            }
                        }
                    }
                    Ok(None) => {
                        log.line("parakit: no speech detected");
                        state.set_phase("idle");
                        sounds.success();
                    }
                    Err(e) => {
                        completion.fail(format!("{e:#}"));
                        log.error(&format!("transcribe failed: {e:#}"));
                        state.set_phase("idle");
                        sounds.error();
                    }
                }
            }
        }
    }
}

/// Always acknowledge a validation job, including early returns and errors.
struct Completion {
    sender: Option<Sender<Result<(), String>>>,
    result: Result<(), String>,
}

impl Completion {
    fn new(sender: Option<Sender<Result<(), String>>>) -> Self {
        Self {
            sender,
            result: Err("dictation produced no transcript".into()),
        }
    }

    fn succeed(&mut self) {
        self.result = Ok(());
    }

    fn fail(&mut self, message: String) {
        self.result = Err(message);
    }
}

impl Drop for Completion {
    fn drop(&mut self) {
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(std::mem::replace(&mut self.result, Ok(())));
        }
    }
}

fn transcribe_clean(
    engine: &Engine,
    pcm: &[f32],
    cleaner: Option<&Cleaner>,
) -> Result<Option<TranscriptResult>> {
    let infer_started = Instant::now();
    let raw = engine.transcribe(pcm)?;
    let infer_elapsed = infer_started.elapsed();
    if raw.trim().is_empty() {
        return Ok(None);
    }

    let clean_started = Instant::now();
    // `Cleaner::clean` fails open: a runtime cleaning failure yields the
    // original transcript plus a description, never a partially transformed
    // string and never a panic on the worker thread.
    let (cleaned, rules_fired, cleaning_failure) = match cleaner {
        Some(c) => {
            let result = c.clean(&raw);
            (result.text, result.rules_fired, result.failure)
        }
        None => (raw.clone(), Vec::new(), None),
    };
    Ok(Some(TranscriptResult {
        raw,
        cleaned,
        rules_fired,
        cleaning_failure,
        infer_elapsed,
        clean_elapsed: clean_started.elapsed(),
    }))
}

/// Reject effectively silent captures before ASR can turn background noise
/// into text. This is amplitude-based: short speech still reaches
/// `Engine::transcribe`, where short captures are padded for the model.
fn capture_should_skip(pcm: &[f32]) -> bool {
    if pcm.is_empty() {
        return true;
    }

    let mut peak = 0.0_f32;
    let mut sum_squares = 0.0_f64;
    for sample in pcm {
        peak = peak.max(sample.abs());
        sum_squares += f64::from(*sample) * f64::from(*sample);
    }
    let rms = (sum_squares / pcm.len() as f64).sqrt() as f32;
    peak < SILENCE_PEAK_THRESHOLD && rms < SILENCE_RMS_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn silence_gate_skips_empty_and_quiet_audio_only() {
        assert!(capture_should_skip(&[]));
        assert!(capture_should_skip(&[0.0; 160]));
        assert!(capture_should_skip(&[0.0001; 160]));
        assert!(!capture_should_skip(&[0.0, 0.2]));
        assert!(!capture_should_skip(&[0.01; 16]));
    }
}
