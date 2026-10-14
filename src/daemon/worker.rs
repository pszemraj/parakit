//! Transcription worker and paste safety boundary.

use anyhow::Result;
use crossbeam_channel::{Receiver, Sender};
use parakit::data_log::{CleaningLogFields, DataLogger};
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
pub(crate) use insertion::{insert_text, FocusCheck, InsertOutcome};
use insertion::{insertion_result_remembers_transcript, log_insertion_outcome, prepared_injector};

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
}

impl WorkerEngine for Engine {
    fn transcribe(&self, pcm: &[f32]) -> Result<String> {
        Self::transcribe(self, pcm)
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
        recipe: _,
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
                                    report.failure_reason.clone(),
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
    use super::super::model_lifecycle::Residency;
    use super::super::sounds::Cue;
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeEngine {
        transcriptions: Arc<AtomicUsize>,
        drops: Arc<AtomicUsize>,
        transcript: std::result::Result<&'static str, &'static str>,
    }

    impl WorkerEngine for FakeEngine {
        fn transcribe(&self, _pcm: &[f32]) -> Result<String> {
            self.transcriptions.fetch_add(1, Ordering::SeqCst);
            self.transcript
                .map(str::to_owned)
                .map_err(anyhow::Error::msg)
        }
    }

    impl Drop for FakeEngine {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn test_worker_ctx(
        engine: FakeEngine,
        state: &Arc<SharedState>,
        sounds: Sounds,
        rx: Receiver<WorkerEvent>,
        model_idle_minutes: u64,
    ) -> WorkerCtx<FakeEngine> {
        let log = Arc::new(Logger::new(super::super::logging::LogLevel::Quiet));
        WorkerCtx {
            engine,
            recipe: EngineRecipe {
                model_path: std::path::PathBuf::new(),
                threads: 1,
                qwen: parakit::qwen::QwenOptions::default(),
                device_mode: parakit::inference::DeviceMode::Cpu,
                verbose: false,
            },
            settings: WorkerSettings {
                model_idle_minutes,
                data_log: None,
                paste_mode: PasteMode::Standard,
                keep_transcript_clipboard: false,
            },
            cleaner: None,
            sounds,
            notifier: Notifier::silent(Arc::clone(&log)),
            log,
            state: Arc::clone(state),
            insert_transcripts: false,
            rx,
            lifetime: state.shutdown.register(),
        }
    }

    fn next_cue(cues: &Receiver<Cue>) -> Cue {
        cues.recv_timeout(Duration::from_secs(1)).unwrap()
    }

    fn stopped_capture(
        now: Instant,
        pcm: Vec<f32>,
        activity: Option<ActivityGuard>,
        completion: Sender<Result<(), String>>,
    ) -> WorkerEvent {
        WorkerEvent::Stopped {
            started_at: now,
            stopped_at: now,
            pcm,
            focus_at_start: None,
            activity,
            completion: Some(completion),
        }
    }

    fn wait_for_state(state: &SharedState, residency: Residency) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while state
            .model_status()
            .is_none_or(|status| status.residency != residency)
        {
            assert!(
                Instant::now() < deadline,
                "worker did not reach {residency:?}"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn silence_gate_skips_empty_and_quiet_audio_only() {
        assert!(capture_should_skip(&[]));
        assert!(capture_should_skip(&[0.0; 160]));
        assert!(capture_should_skip(&[0.0001; 160]));
        assert!(!capture_should_skip(&[0.0, 0.2]));
        assert!(!capture_should_skip(&[0.01; 16]));
    }

    #[test]
    fn worker_plays_error_cue_when_no_speech_is_detected() {
        // Silence stops at the gate; audible noise reaches inference and comes
        // back blank. Neither inserts text, so neither may sound successful.
        for (pcm, inferred) in [(vec![0.0; 160], 0), (vec![0.01; 16], 1)] {
            let state = Arc::new(SharedState::with_history_limit(2));
            state.activity.ready();
            let (tx, rx) = crossbeam_channel::bounded(WORKER_QUEUE_CAPACITY);
            let (sounds, cues) = Sounds::test_channel();
            let transcriptions = Arc::new(AtomicUsize::new(0));
            let ctx = test_worker_ctx(
                FakeEngine {
                    transcriptions: Arc::clone(&transcriptions),
                    drops: Arc::new(AtomicUsize::new(0)),
                    transcript: Ok(" "),
                },
                &state,
                sounds,
                rx,
                0,
            );
            let worker = std::thread::spawn(move || {
                worker_loop_with(ctx, None, || -> Result<FakeEngine> {
                    anyhow::bail!("resident model must not reload")
                });
            });

            let now = Instant::now();
            let (done_tx, done_rx) = crossbeam_channel::bounded(1);
            tx.send(stopped_capture(now, pcm, None, done_tx)).unwrap();
            assert_eq!(
                done_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                Err("dictation produced no transcript".to_string())
            );
            assert!(matches!(next_cue(&cues), Cue::Error));
            drop(tx);
            worker.join().unwrap();
            assert!(cues.try_recv().is_err());
            assert_eq!(transcriptions.load(Ordering::SeqCst), inferred);
            assert!(state.resolve_transcript(0).is_err());
        }
    }

    #[test]
    fn worker_loop_offloads_then_reloads_and_transcribes_queued_capture() {
        exercise_blocked_reload(ReloadCase::Released);
    }

    #[test]
    fn failed_transcription_keeps_previous_history_and_reports_failure() {
        let state = Arc::new(SharedState::with_history_limit(2));
        state.activity.ready();
        state.remember_transcript("previous dictation".to_owned());
        let (tx, rx) = crossbeam_channel::bounded(WORKER_QUEUE_CAPACITY);
        let (sounds, cues) = Sounds::test_channel();
        let ctx = test_worker_ctx(
            FakeEngine {
                transcriptions: Arc::new(AtomicUsize::new(0)),
                drops: Arc::new(AtomicUsize::new(0)),
                transcript: Err("incomplete native decoding"),
            },
            &state,
            sounds,
            rx,
            0,
        );
        let worker = std::thread::spawn(move || {
            worker_loop_with(ctx, None, || panic!("resident model must not reload"))
        });
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let capture = stopped_capture(Instant::now(), vec![0.1; 160], None, done_tx);
        tx.send(capture).unwrap();
        assert!(done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap_err()
            .contains("incomplete native decoding"));
        assert!(matches!(next_cue(&cues), Cue::Error));
        drop(tx);
        worker.join().unwrap();
        assert_eq!(state.resolve_transcript(0).unwrap(), "previous dictation");
        assert!(state.resolve_transcript(1).is_err());
    }

    #[test]
    fn worker_announces_reload_then_readiness_while_ptt_held() {
        exercise_blocked_reload(ReloadCase::Held);
    }

    #[test]
    fn worker_failed_reload_never_announces_readiness() {
        exercise_blocked_reload(ReloadCase::Failed);
    }

    #[test]
    fn worker_retries_transient_reload_failure_after_recording_stops() {
        exercise_blocked_reload(ReloadCase::Recovered);
    }

    #[test]
    fn worker_reports_immediate_retry_after_an_early_release() {
        exercise_blocked_reload(ReloadCase::ReleasedRecovered);
    }

    #[test]
    fn worker_shutdown_during_reload_drops_session_and_queued_capture() {
        exercise_blocked_reload(ReloadCase::Shutdown);
    }

    #[derive(Clone, Copy, PartialEq)]
    enum ReloadCase {
        Held,
        Released,
        Failed,
        Recovered,
        ReleasedRecovered,
        Shutdown,
    }

    fn exercise_blocked_reload(case: ReloadCase) {
        let state = Arc::new(SharedState::with_history_limit(2));
        state.activity.ready();
        let (tx, rx) = crossbeam_channel::bounded(WORKER_QUEUE_CAPACITY);
        let reloads = Arc::new(AtomicUsize::new(0));
        let transcriptions = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let loader_activity = Arc::clone(&state.activity);
        let loader_reloads = Arc::clone(&reloads);
        let loader_transcriptions = Arc::clone(&transcriptions);
        let loader_drops = Arc::clone(&drops);
        let (load_started_tx, load_started_rx) = crossbeam_channel::bounded(1);
        let (load_release_tx, load_release_rx) = crossbeam_channel::bounded(1);
        let (sounds, cues) = Sounds::test_channel();
        let ctx = test_worker_ctx(
            FakeEngine {
                transcriptions: Arc::clone(&transcriptions),
                drops: Arc::clone(&drops),
                transcript: Ok("reloaded dictation"),
            },
            &state,
            sounds,
            rx,
            1,
        );

        let worker = std::thread::spawn(move || {
            let load = move || {
                // A reload may admit other activity; it must not inherit the
                // gate mutex held by the offload decision.
                let _admission = loader_activity.begin();
                let attempt = loader_reloads.fetch_add(1, Ordering::SeqCst);
                if attempt == 0 {
                    load_started_tx.send(()).unwrap();
                    load_release_rx.recv().unwrap();
                }
                if case == ReloadCase::Failed
                    || (matches!(case, ReloadCase::Recovered | ReloadCase::ReleasedRecovered)
                        && attempt == 0)
                {
                    anyhow::bail!("reload failed in test");
                }
                Ok(FakeEngine {
                    transcriptions: Arc::clone(&loader_transcriptions),
                    drops: Arc::clone(&loader_drops),
                    transcript: Ok("reloaded dictation"),
                })
            };
            worker_loop_with(ctx, Some(Duration::from_millis(10)), load);
        });

        wait_for_state(&state, Residency::Offloaded);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(reloads.load(Ordering::SeqCst), 0);
        assert!(cues.try_recv().is_err(), "offloading alone is silent");

        // Keep the reloaded model resident until all post-completion checks
        // have observed it; the capture lease moves into `Stopped` below.
        let verify_residency = state.activity.begin();
        let recording = state.activity.begin();
        let (completion_tx, completion_rx) = crossbeam_channel::bounded(1);
        let now = Instant::now();
        let capture_active = Arc::new(AtomicBool::new(true));
        tx.send(WorkerEvent::Started {
            recording: Arc::clone(&capture_active),
        })
        .unwrap();
        load_started_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        assert!(matches!(next_cue(&cues), Cue::Reload));
        assert!(
            cues.try_recv().is_err(),
            "must not announce readiness during reload"
        );
        let stopped = stopped_capture(now, vec![0.01; 16], Some(recording), completion_tx);
        // The released capture remains queued; a held capture stops only after
        // readiness is heard below.
        let held_capture = if matches!(
            case,
            ReloadCase::Held | ReloadCase::Failed | ReloadCase::Recovered
        ) {
            Some(stopped)
        } else {
            capture_active.store(false, Ordering::Release);
            tx.send(stopped).unwrap();
            None
        };
        let completion_before_reload = completion_rx.try_recv();
        if case == ReloadCase::Shutdown {
            assert_eq!(state.model_status().unwrap().residency, Residency::Loading);
            assert!(!state
                .shutdown
                .request_and_wait(&state.activity, Duration::ZERO));
        }
        load_release_tx.send(()).unwrap();
        if let Some(stopped) = held_capture {
            if case == ReloadCase::Held {
                assert!(matches!(next_cue(&cues), Cue::Start(_)));
            } else {
                wait_for_state(&state, Residency::Offloaded);
                assert_eq!(reloads.load(Ordering::SeqCst), 1);
                assert!(matches!(next_cue(&cues), Cue::Error));
                assert!(
                    cues.try_recv().is_err(),
                    "failure must not announce readiness"
                );
            }
            capture_active.store(false, Ordering::Release);
            tx.send(stopped).unwrap();
        } else if case == ReloadCase::ReleasedRecovered {
            assert!(matches!(next_cue(&cues), Cue::Error));
        }
        assert_eq!(
            completion_before_reload,
            Err(crossbeam_channel::TryRecvError::Empty)
        );

        if case == ReloadCase::Shutdown {
            assert!(state
                .shutdown
                .request_and_wait(&state.activity, Duration::from_secs(1)));
            // Shutdown must not report completion with a live native session.
            assert_eq!(drops.load(Ordering::SeqCst), 2);
            worker.join().unwrap();
            assert!(
                cues.try_recv().is_err(),
                "shutdown must not announce readiness"
            );
            // The producer owns the channel's remaining queued payloads until
            // it also stops; no receiver may process them after worker exit.
            drop(tx);
            assert_eq!(reloads.load(Ordering::SeqCst), 1);
            assert_eq!(transcriptions.load(Ordering::SeqCst), 0);
            assert!(state.resolve_transcript(0).is_err());
            assert_eq!(
                completion_rx.try_recv(),
                Err(crossbeam_channel::TryRecvError::Disconnected)
            );
            drop(verify_residency);
            assert_eq!(
                state.activity.remaining(Some(Duration::ZERO)),
                Some(Duration::ZERO)
            );
            return;
        }

        let completion = completion_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            reloads.load(Ordering::SeqCst),
            if matches!(
                case,
                ReloadCase::Failed | ReloadCase::Recovered | ReloadCase::ReleasedRecovered
            ) {
                2
            } else {
                1
            }
        );
        if case == ReloadCase::Failed {
            assert!(completion.unwrap_err().contains("reload failed in test"));
            assert_eq!(transcriptions.load(Ordering::SeqCst), 0);
            assert_eq!(
                state.model_status().unwrap().residency,
                Residency::Offloaded
            );
            assert!(state.resolve_transcript(0).is_err());
            assert!(matches!(next_cue(&cues), Cue::Error));
        } else {
            assert_eq!(completion, Ok(()));
            assert_eq!(transcriptions.load(Ordering::SeqCst), 1);
            assert_eq!(state.model_status().unwrap().residency, Residency::Loaded);
            assert_eq!(state.resolve_transcript(0).unwrap(), "reloaded dictation");
            assert!(matches!(next_cue(&cues), Cue::Success));
        }
        assert!(state.resolve_transcript(1).is_err());

        if case == ReloadCase::Held {
            // A later PTT with the model resident needs only the normal cue.
            let next = Arc::new(AtomicBool::new(true));
            tx.send(WorkerEvent::Started {
                recording: Arc::clone(&next),
            })
            .unwrap();
            assert!(matches!(next_cue(&cues), Cue::Start(_)));
            next.store(false, Ordering::Release);
            let (done_tx, done_rx) = crossbeam_channel::bounded(1);
            tx.send(stopped_capture(
                now,
                vec![0.01; 16],
                Some(state.activity.begin()),
                done_tx,
            ))
            .unwrap();
            assert_eq!(
                done_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                Ok(())
            );
            assert!(matches!(next_cue(&cues), Cue::Success));
            assert_eq!(reloads.load(Ordering::SeqCst), 1);
            assert_eq!(transcriptions.load(Ordering::SeqCst), 2);
        }

        drop(verify_residency);
        drop(tx);
        worker.join().unwrap();
        assert_eq!(
            completion_rx.try_recv(),
            Err(crossbeam_channel::TryRecvError::Disconnected)
        );
        assert!(cues.try_recv().is_err());
        assert_eq!(
            drops.load(Ordering::SeqCst),
            if case == ReloadCase::Failed { 1 } else { 2 }
        );
    }
}
