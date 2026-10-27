//! Transcription worker and paste safety boundary.

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use parakit::data_log::{CleaningLogFields, DataLogger, InferenceLogFields};
use parakit::inference::Engine;
use parakit::rules::{Cleaner, RuleHit, CLEANER_VERSION};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
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
#[cfg(not(target_os = "linux"))]
use insertion::prepared_injector;
pub(crate) use insertion::{insert_text, FocusCheck, InsertOutcome};
use insertion::{insertion_result_remembers_transcript, log_insertion_outcome};

/// Maximum number of worker events that may queue while ASR or paste is busy.
pub(crate) const WORKER_QUEUE_CAPACITY: usize = 2;

const SILENCE_PEAK_THRESHOLD: f32 = 0.001;
const SILENCE_RMS_THRESHOLD: f32 = 0.0005;

/// Events consumed by the transcription worker.
pub(crate) enum WorkerEvent {
    /// Recording began at this instant.
    Started {
        /// Cleared on release so readiness cannot announce an ended capture.
        recording: Arc<AtomicBool>,
    },
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

/// Effective settings shared by daemon and WAV-simulation workers.
pub(crate) struct WorkerSettings {
    /// Effective user setting, also exposed in status.
    pub(crate) model_idle_minutes: u64,
    /// Optional transcription metadata logger.
    pub(crate) data_log: Option<Arc<DataLogger>>,
    /// Paste chord mode.
    pub(crate) paste_mode: PasteMode,
    /// Leave transcript text on the clipboard after paste/fallback instead of
    /// restoring previous supported clipboard contents.
    pub(crate) keep_transcript_clipboard: bool,
}

/// Dependencies owned by the transcription worker thread.
pub(crate) struct WorkerCtx<E = Engine> {
    /// Open transcription engine.
    pub(crate) engine: E,
    /// Startup parameters reused when the model has been offloaded.
    pub(crate) recipe: EngineRecipe,
    /// Shared resolved configuration.
    pub(crate) settings: WorkerSettings,
    /// Optional transcript cleaner.
    pub(crate) cleaner: Option<Arc<Cleaner>>,
    /// Audio cue player.
    pub(crate) sounds: Sounds,
    /// Process logger.
    pub(crate) log: Arc<Logger>,
    /// Desktop notification helper.
    pub(crate) notifier: Notifier,
    /// Shared state exposed through local IPC.
    pub(crate) state: Arc<SharedState>,
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

/// Minimal worker-facing inference surface, kept private so production uses the
/// concrete engine while the lifecycle loop can be exercised without a model.
trait WorkerEngine {
    /// Transcribe one owned PCM capture.
    ///
    /// # Returns
    ///
    /// The raw transcript from the inference session.
    ///
    /// # Errors
    ///
    /// Reports session or inference failures to the worker's completion path.
    fn transcribe(&self, pcm: &[f32]) -> Result<String>;

    /// Identity of the current session, absent for model-free test engines.
    ///
    /// # Returns
    /// Optional fields to append to a successful transcription's JSONL row.
    fn inference_metadata(&self, _recipe: &EngineRecipe) -> Option<InferenceLogFields> {
        None
    }
}

impl WorkerEngine for Engine {
    fn transcribe(&self, pcm: &[f32]) -> Result<String> {
        Self::transcribe(self, pcm)
    }

    fn inference_metadata(&self, recipe: &EngineRecipe) -> Option<InferenceLogFields> {
        Some(InferenceLogFields {
            model: recipe
                .model_path
                .file_name()?
                .to_string_lossy()
                .into_owned(),
            model_preset: recipe.model_preset.map(|preset| preset.id().to_owned()),
            backend: self.backend().to_owned(),
            quantization: parakit::gguf::dtype_label(&recipe.model_path),
            requested_device: recipe.device_mode,
            session_device: self.device_mode(),
            confirmed_device: (self.device_mode() == parakit::inference::DeviceMode::Cpu)
                .then(|| "cpu".to_owned()),
            qwen_language: self.qwen_options().map(|options| {
                options
                    .normalized_language()
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| "auto".to_owned())
            }),
            qwen_max_new_tokens: self.qwen_options().map(|options| options.max_new_tokens),
            native_api_version: self.native_api_version(),
        })
    }
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
    let timeout = super::model_lifecycle::idle_timeout(ctx.settings.model_idle_minutes);
    let recipe = ctx.recipe.clone();
    let reload_log = Arc::clone(&ctx.log);
    let reload_state = Arc::clone(&ctx.state);
    let reload_notifier = ctx.notifier.clone();
    let load = move || {
        let (engine, device) = recipe.reload(&reload_log)?;
        // Reload changes the device only when `auto` lacks GPU room; its
        // stderr warning is invisible to a daemon started in the background.
        if engine.device_mode() != recipe.device_mode {
            reload_notifier.model_reloaded_on_cpu();
        }
        reload_state.update_engine_info(engine.backend(), device);
        Ok(engine)
    };
    worker_loop_with(ctx, timeout, load);
}

/// Run the production worker event loop with an opened session and reload path.
fn worker_loop_with<E, Load>(ctx: WorkerCtx<E>, timeout: Option<Duration>, mut load: Load)
where
    E: WorkerEngine,
    Load: FnMut() -> Result<E>,
{
    let WorkerCtx {
        engine,
        recipe,
        settings:
            WorkerSettings {
                model_idle_minutes,
                data_log,
                paste_mode,
                keep_transcript_clipboard,
            },
        cleaner,
        sounds,
        log,
        notifier,
        state,
        insert_transcripts,
        rx,
        lifetime: _lifetime,
    } = ctx;
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
    #[cfg(target_os = "linux")]
    let mut injector = None;
    #[cfg(not(target_os = "linux"))]
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
            WorkerEvent::Started { recording } => {
                state.set_phase("recording");
                log.line("parakit: recording...");
                if !model.is_loaded() {
                    sounds.reload();
                }
                model.ensure_loaded(&mut load, |status| state.set_model_status(status));
                if !model.is_loaded() && !state.shutdown.requested() {
                    let error = model
                        .status()
                        .last_error
                        .unwrap_or_else(|| "model reload failed for an unknown reason".to_string());
                    log.error(&format!("{error}; reload will retry before transcription"));
                    sounds.error();
                    notifier.model_unavailable(&error);
                }
                if model.is_loaded()
                    && recording.load(Ordering::Acquire)
                    && !state.shutdown.requested()
                {
                    sounds.start(recording);
                }
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
                // A transient reload failure at PTT start must not discard the
                // capture: try once more now that recording has finished.
                model.ensure_loaded(&mut load, |status| state.set_model_status(status));
                if state.shutdown.requested() {
                    break;
                }
                if model.engine().is_ok() && capture_should_skip(&pcm) {
                    log.verbose(format!(
                        "parakit: skipped silent capture ({secs:.2}s audio, {wall_secs:.2}s wall)"
                    ));
                    log.line("parakit: no speech detected");
                    state.set_phase("idle");
                    // Nothing reaches the target, so a success cue would mislead.
                    sounds.error();
                    continue;
                }
                let engine = match model.engine() {
                    Ok(engine) => engine,
                    Err(error) => {
                        let error = format!("{error:#}");
                        drop(pcm);
                        completion.fail(error.clone());
                        log.error(&format!("dictation discarded: {error}"));
                        state.set_phase("idle");
                        // The press-time cue reported the first load failure;
                        // this cue reports the distinct release retry failure
                        // that discards the completed capture.
                        sounds.error();
                        notifier.dictation_discarded(&error);
                        continue;
                    }
                };
                state.set_phase("transcribing");
                log.transcribing(secs, wall_secs);

                let transcription = transcribe_clean(engine, &pcm, cleaner.as_deref());
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
                                engine.inference_metadata(&recipe),
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
                                paste_mode,
                            );
                            state.remember_transcript(transcript.cleaned);
                            state.set_phase("idle");
                            sounds.success();
                            continue;
                        }
                        let insert_started = Instant::now();
                        let cleaned = transcript.cleaned;
                        let focus_verification = Cell::new("not_applicable");
                        let focus_check = FocusCheck {
                            snapshot: focus_at_start.as_deref(),
                            verification: &focus_verification,
                        };
                        let (insert_result, remembers_transcript) =
                            state.with_insertion_lock(|| {
                                let result = insert_text(
                                    &mut injector,
                                    &cleaned,
                                    paste_mode,
                                    keep_transcript_clipboard,
                                    focus_check,
                                    (log.as_ref(), &notifier),
                                );
                                let remembers_transcript =
                                    insertion_result_remembers_transcript(&result);
                                if remembers_transcript {
                                    state.remember_transcript(cleaned);
                                }
                                (result, remembers_transcript)
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
                                    report.failure_reason.clone(),
                                    focus_verification.get(),
                                    Some(&report),
                                    paste_mode,
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
                                    if remembers_transcript {
                                        log_recovery_hint(&state, &log);
                                    }
                                    sounds.error();
                                } else {
                                    sounds.success();
                                }
                            }
                            Err(e) => {
                                let report = insertion::insertion_error_report(&e);
                                log_insertion_outcome(
                                    &data_log,
                                    record_id,
                                    "error",
                                    focus_at_start.as_deref(),
                                    transcript_chars,
                                    Some(format!("{e:#}")),
                                    focus_verification.get(),
                                    report,
                                    paste_mode,
                                );
                                log.error(&format!("paste failed: {e:#}"));
                                log_recovery_hint(&state, &log);
                                state.set_phase("idle");
                                sounds.error();
                            }
                        }
                    }
                    Ok(None) => {
                        log.line("parakit: no speech detected");
                        state.set_phase("idle");
                        sounds.error();
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

fn transcribe_clean<E: WorkerEngine>(
    engine: &E,
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

/// Explain recovery only when the dictation was actually retained.
fn log_recovery_hint(state: &SharedState, log: &Logger) {
    if state.resolve_transcript(0).is_ok() {
        log.warn("transcript saved; check the target before retrying. Recover it with `parakit copy-last`");
    }
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
mod tests;
