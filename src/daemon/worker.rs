//! Transcription worker and paste safety boundary.

use anyhow::{Context, Result};
use crossbeam_channel::Receiver;
use parakit::data_log::{CleaningLogFields, DataLogger, InsertionLogFields, RecordId};
use parakit::inference::Engine;
use parakit::rules::{Cleaner, RuleHit, CLEANER_VERSION};
use std::cell::Cell;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::audio::TARGET_RATE;
use super::inject::{ClipboardPolicy, FocusSnapshot, FocusVerification, Injector, PasteMode};
use super::ipc::SharedState;
use super::logging::Logger;
use super::notifications::Notifier;
use super::sounds::Sounds;

/// Maximum number of worker events that may queue while ASR or paste is busy.
pub(crate) const WORKER_QUEUE_CAPACITY: usize = 2;

const PASTE_FAILURE_CIRCUIT_BREAKER: usize = 3;
const PASTE_FAILURE_RESET_AFTER: Duration = Duration::from_secs(60);
const MAX_PASTE_CHARS: usize = 20_000;
const TERMINAL_MAX_PASTE_CHARS: usize = 2_000;
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
    },
}

/// Dependencies owned by the transcription worker thread.
pub(crate) struct WorkerCtx {
    /// Open transcription engine.
    pub(crate) engine: Engine,
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
    } = ctx;

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
    let number_threshold = cleaner.as_deref().and_then(Cleaner::number_threshold);
    let mut injector = if insert_transcripts {
        match Injector::new() {
            Ok(mut injector) => match injector.prepare_for_mode(paste_mode) {
                Ok(()) => Some(injector),
                Err(err) => {
                    log.error(&format!(
                        "insertion backend unavailable at worker startup: {err:#}"
                    ));
                    None
                }
            },
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
    let mut paste_circuit = PasteCircuit::default();
    while let Ok(ev) = rx.recv() {
        match ev {
            WorkerEvent::Started => {
                state.set_phase("recording");
                sounds.start();
                log.line("parakit: recording...");
            }
            WorkerEvent::Failed { message } => {
                log.error(&message);
                state.set_phase("idle");
                sounds.error();
            }
            WorkerEvent::Stopped {
                started_at,
                stopped_at,
                pcm,
                focus_at_start,
            } => {
                let stop_started = Instant::now();
                let drain_elapsed = stop_started.saturating_duration_since(stopped_at);
                let secs = pcm.len() as f32 / TARGET_RATE as f32;
                let wall_secs = stopped_at.duration_since(started_at).as_secs_f32();
                if capture_should_skip(&pcm) {
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

                match transcribe_clean(&engine, &pcm, cleaner.as_deref()) {
                    Ok(Some(transcript)) => {
                        // Count the dictation itself, once, regardless of how
                        // many insertion attempts or outcomes follow below.
                        state.record_dictation();
                        if let Some(failure) = transcript.cleaning_failure.as_deref() {
                            log.warn(format!(
                                "parakit: cleaning failed, inserting the raw transcript: {failure}"
                            ));
                        }
                        let record_id = data_log.as_ref().map(|data_log| {
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
                        paste_circuit.maybe_reenable(Instant::now(), log.as_ref());
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
                                paste_circuit.copy_only_mode,
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
                                if matches!(
                                    outcome,
                                    InsertOutcome::Pasted | InsertOutcome::PastedUnverified
                                ) {
                                    paste_circuit.record_success();
                                }
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
                                // Blocked always alarms. A post-chord
                                // CopiedOnly (paste sent but insertion never
                                // confirmed) is a safe degradation, not a
                                // backend failure, but the user still needs
                                // to know the paste did not land; a
                                // pre-chord CopiedOnly (guard blocked before
                                // any chord was sent) keeps the quieter
                                // existing behavior.
                                let needs_alert = matches!(outcome, InsertOutcome::Blocked)
                                    || (outcome == InsertOutcome::CopiedOnly
                                        && report.paste_event_posted);
                                if needs_alert {
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
                                if paste_circuit.record_failure(Instant::now()) {
                                    notifier.paste_temporarily_disabled();
                                }
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
                        log.error(&format!("transcribe failed: {e:#}"));
                        state.set_phase("idle");
                        sounds.error();
                    }
                }
            }
        }
    }
}

#[derive(Default)]
struct PasteCircuit {
    consecutive_failures: usize,
    last_failure: Option<Instant>,
    copy_only_mode: bool,
}

impl PasteCircuit {
    fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.last_failure = None;
    }

    fn record_failure(&mut self, now: Instant) -> bool {
        if self.last_failure.is_some_and(|last_failure| {
            now.saturating_duration_since(last_failure) >= PASTE_FAILURE_RESET_AFTER
        }) {
            self.consecutive_failures = 0;
            self.copy_only_mode = false;
        }
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        self.last_failure = Some(now);
        if self.consecutive_failures >= PASTE_FAILURE_CIRCUIT_BREAKER && !self.copy_only_mode {
            self.copy_only_mode = true;
            return true;
        }
        false
    }

    fn maybe_reenable(&mut self, now: Instant, log: &Logger) {
        if !self.copy_only_mode {
            return;
        }
        let Some(last_failure) = self.last_failure else {
            return;
        };
        if now.saturating_duration_since(last_failure) < PASTE_FAILURE_RESET_AFTER {
            return;
        }
        self.consecutive_failures = 0;
        self.last_failure = None;
        self.copy_only_mode = false;
        log.line("parakit: paste cooldown elapsed; automatic paste re-enabled");
    }
}

fn insertion_result_remembers_transcript(result: &Result<InsertReport>) -> bool {
    !matches!(
        result,
        Ok(InsertReport {
            outcome: InsertOutcome::Skipped,
            ..
        })
    )
}

/// Write an insertion-outcome telemetry record correlated with the
/// transcription record `record_id` refers to.
///
/// A no-op when `data_log` or `record_id` is `None`, which happens whenever
/// data logging is disabled or the transcription record itself failed to log.
///
/// # Arguments
///
/// * `data_log` - Optional shared transcription logger.
/// * `record_id` - Identifier of the transcription record to correlate with.
/// * `outcome` - Coarse insertion result label.
/// * `focus_at_start` - Focus captured before insertion became eligible, used
///   to recover a target bundle identifier on platforms that expose one.
/// * `transcript_chars` - Character count of the transcript offered for
///   insertion.
/// * `failure_reason` - Error display text when `outcome` is `"error"`.
/// * `focus_verification` - How focus was verified before insertion, as
///   observed by the last live recheck performed for this attempt (or
///   `"not_applicable"` when insertion never reached a focus check, e.g.
///   sanitizer skip/copy-only paths and PTT audio simulation).
/// * `report` - Real acknowledgement/clipboard telemetry for this attempt,
///   when one is available (every `Ok` insertion result has one; `None` for
///   PTT audio simulation and the `"error"` outcome, which have nothing to
///   report).
#[allow(
    clippy::too_many_arguments,
    reason = "each parameter is an independently observed fact about one insertion (logger, \
              record id, outcome, focus at start, transcript length, failure reason, focus \
              verification, and paste report); the three call sites each learn these at \
              different points, so bundling them into a struct would only move the argument \
              list to the construction site"
)]
fn log_insertion_outcome(
    data_log: &Option<Arc<DataLogger>>,
    record_id: Option<RecordId>,
    outcome: &'static str,
    focus_at_start: Option<&FocusSnapshot>,
    transcript_chars: usize,
    failure_reason: Option<String>,
    focus_verification: &'static str,
    report: Option<&InsertReport>,
) {
    let (Some(data_log), Some(record_id)) = (data_log, record_id) else {
        return;
    };
    let target_bundle_id = focus_at_start.and_then(FocusSnapshot::target_bundle_id);
    data_log.log_insertion(
        record_id,
        InsertionLogFields {
            outcome,
            target_bundle_id,
            focus_verification,
            transcript_chars,
            paste_event_posted: report.map_or(outcome == "pasted", |r| r.paste_event_posted),
            // NSPasteboard/Carbon promise-keeper read evidence is a
            // deferred follow-up (see `daemon::macos::pasteboard` module
            // docs); no call site can populate this yet.
            pasteboard_requested: None,
            acknowledgement_kind: report.map_or("not_applicable", |r| r.acknowledgement_kind),
            acknowledgement_ms: report.and_then(|r| r.acknowledgement_ms),
            clipboard_restored: report.and_then(|r| r.clipboard_restored),
            failure_reason: failure_reason.as_deref(),
        },
    );
}

/// Focus snapshot captured before insertion became eligible, paired with the
/// telemetry cell that records the label of the last live focus recheck
/// performed against it. Bundled together because every insertion call site
/// that reads one also updates the other.
#[derive(Clone, Copy)]
pub(crate) struct FocusCheck<'a> {
    /// Focus captured at PTT-down, when capture succeeded.
    pub(crate) snapshot: Option<&'a FocusSnapshot>,
    /// Updated with the telemetry label of the last live focus recheck
    /// performed against `snapshot`. Left at its initial value when
    /// insertion never reaches a focus check (e.g. sanitizer skip/copy-only
    /// paths).
    pub(crate) verification: &'a Cell<&'static str>,
}

/// Sanitize text and run the shared paste/copy insertion transaction.
///
/// # Arguments
///
/// * `injector` - Reused insertion backend, created lazily when needed.
/// * `raw_text` - Candidate transcript or IPC text.
/// * `mode` - Paste mode used for sanitizer policy and chord selection.
/// * `keep_transcript_clipboard` - Leave text on clipboard instead of restoring previous contents.
/// * `focus` - Focus snapshot and telemetry cell (see [`FocusCheck`]).
/// * `ui` - Daemon logger and desktop notification wrapper.
/// * `copy_only_mode` - Circuit-breaker flag that disables synthetic paste.
///
/// # Returns
///
/// Worker insertion outcome plus acknowledgement/clipboard telemetry.
///
/// # Errors
///
/// Returns an error if clipboard staging, injector initialization, or paste fails.
pub(crate) fn insert_text(
    injector: &mut Option<Injector>,
    raw_text: &str,
    mode: PasteMode,
    keep_transcript_clipboard: bool,
    focus: FocusCheck<'_>,
    ui: (&Logger, &Notifier),
    copy_only_mode: bool,
) -> Result<InsertReport> {
    let (log, notifier) = ui;
    match sanitize_for_paste(raw_text, mode) {
        PastePlan::Paste(text) if copy_only_mode => {
            if mode == PasteMode::Direct {
                log.warn(
                    "direct insertion disabled after repeated failures; transcript was not copied",
                );
                notifier.paste_temporarily_disabled();
                Ok(InsertReport::placeholder(InsertOutcome::Blocked, false))
            } else {
                copy_or_block_transcript(
                    injector,
                    &text,
                    keep_transcript_clipboard,
                    "paste-disabled clipboard copy failed",
                    PasteBlockReason::PasteTemporarilyDisabled,
                    log,
                    notifier,
                )
            }
        }
        PastePlan::Paste(text) => paste_transcript(
            injector,
            &text,
            mode,
            keep_transcript_clipboard,
            focus,
            log,
            notifier,
        ),
        PastePlan::CopyOnly { text, reason } => {
            if mode == PasteMode::Direct {
                log.warn(format!(
                    "direct insertion blocked by sanitizer ({}); transcript was not copied",
                    reason.log_tag()
                ));
                notifier.paste_blocked(reason.notice());
                Ok(InsertReport::placeholder(InsertOutcome::Blocked, false))
            } else {
                log.warn(format!("paste blocked by sanitizer ({})", reason.log_tag()));
                copy_or_block_transcript(
                    injector,
                    &text,
                    keep_transcript_clipboard,
                    "sanitized transcript clipboard fallback failed",
                    reason,
                    log,
                    notifier,
                )
            }
        }
        PastePlan::Skip { reason } => {
            log.warn(format!("paste skipped by sanitizer: {}", reason.log_tag()));
            Ok(InsertReport::placeholder(InsertOutcome::Skipped, false))
        }
    }
}

fn paste_transcript(
    injector: &mut Option<Injector>,
    text: &str,
    mode: PasteMode,
    keep_transcript_clipboard: bool,
    focus: FocusCheck<'_>,
    log: &Logger,
    notifier: &Notifier,
) -> Result<InsertReport> {
    if !focus_allows_insertion(focus, log) {
        if mode == PasteMode::Direct {
            notifier.paste_blocked(PasteBlockReason::FocusChangedBeforeInsertion.notice());
            return Ok(InsertReport::placeholder(InsertOutcome::Blocked, false));
        }
        return copy_or_block_transcript(
            injector,
            text,
            keep_transcript_clipboard,
            "focus changed clipboard fallback failed",
            PasteBlockReason::FocusChangedBeforeInsertion,
            log,
            notifier,
        );
    }

    if injector.is_none() {
        *injector = Some(Injector::new().context("could not initialize insertion backend")?);
    }
    let prepare_result = injector
        .as_mut()
        .expect("insertion backend was just initialized")
        .prepare_for_mode(mode);
    if let Err(err) = prepare_result {
        if mode != PasteMode::Direct {
            log.warn(format!(
                "paste backend unavailable ({err:#}); automatic paste skipped"
            ));
            return copy_or_block_transcript(
                injector,
                text,
                keep_transcript_clipboard,
                "paste backend unavailable and clipboard fallback failed",
                PasteBlockReason::BackendUnavailable,
                log,
                notifier,
            );
        }
        return Err(err.context("could not prepare direct insertion backend"));
    }

    let paste_result = injector
        .as_mut()
        .expect("insertion backend was just initialized")
        .paste_text_guarded(
            text,
            mode,
            clipboard_policy(keep_transcript_clipboard),
            focus.snapshot,
            || Ok(focus_allows_insertion(focus, log)),
        );
    let paste_error = match paste_result {
        Ok(report) => match report.outcome {
            super::inject::PasteOutcome::Pasted => {
                return Ok(InsertReport::from_paste(InsertOutcome::Pasted, report))
            }
            super::inject::PasteOutcome::PastedUnverified => {
                log.verbose(format!(
                    "parakit: paste sent but insertion could not be confirmed within {}ms ({}); treating as pasted",
                    report.acknowledgement_ms.unwrap_or_default(),
                    report.acknowledgement_kind,
                ));
                return Ok(InsertReport::from_paste(
                    InsertOutcome::PastedUnverified,
                    report,
                ));
            }
            super::inject::PasteOutcome::CopiedOnly => {
                if report.paste_event_posted {
                    // The paste chord was sent but never confirmed: avoid
                    // alarm fatigue from a second silent failure path and
                    // tell the user the transcript is safe on the
                    // clipboard.
                    notifier.paste_blocked(PasteBlockReason::Unconfirmed.notice());
                } else {
                    notifier.transcript_copied(PasteBlockReason::FocusChangedBeforePaste.notice());
                }
                return Ok(InsertReport::from_paste(InsertOutcome::CopiedOnly, report));
            }
            super::inject::PasteOutcome::Blocked => {
                notifier.paste_blocked(PasteBlockReason::FocusChangedBeforePaste.notice());
                return Ok(InsertReport::from_paste(InsertOutcome::Blocked, report));
            }
        },
        Err(err) => err,
    };

    if paste_failure_uses_clipboard_fallback(mode, &paste_error, keep_transcript_clipboard) {
        copy_transcript_to_clipboard(injector, text).map_err(|copy_error| {
            anyhow::anyhow!("{paste_error:#}; clipboard fallback also failed: {copy_error:#}")
        })?;
        return Err(
            paste_error.context("transcript copied to clipboard fallback after paste failure")
        );
    }

    Err(paste_error)
}

fn copy_or_block_transcript(
    injector: &mut Option<Injector>,
    text: &str,
    keep_transcript_clipboard: bool,
    copy_context: &'static str,
    reason: PasteBlockReason,
    log: &Logger,
    notifier: &Notifier,
) -> Result<InsertReport> {
    match stage_transcript_for_history(injector, text, keep_transcript_clipboard)
        .context(copy_context)?
    {
        super::inject::StageOutcome::CopiedOnly => {
            notifier.transcript_copied(reason.notice());
            Ok(InsertReport::from_stage(InsertOutcome::CopiedOnly, false))
        }
        super::inject::StageOutcome::Blocked => {
            log.warn(format!(
                "automatic paste skipped ({}); transcript staged for clipboard history",
                reason.log_tag()
            ));
            notifier.paste_blocked(reason.notice());
            Ok(InsertReport::from_stage(InsertOutcome::Blocked, true))
        }
    }
}

fn stage_transcript_for_history(
    injector: &mut Option<Injector>,
    text: &str,
    keep_transcript_clipboard: bool,
) -> Result<super::inject::StageOutcome> {
    let policy = clipboard_policy(keep_transcript_clipboard);
    with_injector(injector, |injector| {
        injector.stage_text_for_history(text, policy)
    })
}

fn copy_transcript_to_clipboard(injector: &mut Option<Injector>, text: &str) -> Result<()> {
    with_injector(injector, |injector| injector.copy_text(text))
}

fn with_injector<R>(
    injector: &mut Option<Injector>,
    f: impl FnOnce(&mut Injector) -> Result<R>,
) -> Result<R> {
    if injector.is_none() {
        *injector = Some(
            Injector::new()
                .context("could not initialize insertion backend for clipboard fallback")?,
        );
    }
    f(injector
        .as_mut()
        .expect("insertion backend was just initialized"))
}

fn paste_failure_uses_clipboard_fallback(
    mode: PasteMode,
    error: &anyhow::Error,
    keep_transcript_clipboard: bool,
) -> bool {
    let message = format!("{error:#}");
    keep_transcript_clipboard
        && mode != PasteMode::Direct
        && !message.contains(super::inject::CLIPBOARD_RESTORE_ERROR)
}

fn clipboard_policy(keep_transcript_clipboard: bool) -> ClipboardPolicy {
    if keep_transcript_clipboard {
        ClipboardPolicy::KeepTranscript
    } else {
        ClipboardPolicy::RestorePrevious
    }
}

/// Decide whether insertion may proceed for `focus.snapshot`, recording the
/// telemetry label for this check into `focus.verification`.
///
/// # Arguments
///
/// * `focus` - Focus snapshot captured before insertion became eligible,
///   plus the telemetry cell this check's label is recorded into:
///   `"unavailable"` when no snapshot was captured at PTT-down, or the
///   platform's live verification label otherwise (see
///   [`FocusSnapshot::verify_current`]).
/// * `log` - Daemon logger used for diagnostics.
fn focus_allows_insertion(focus: FocusCheck<'_>, log: &Logger) -> bool {
    let Some(snapshot) = focus.snapshot else {
        focus.verification.set("unavailable");
        if cfg!(any(target_os = "macos", target_os = "windows")) {
            // macOS and Windows insertion must prove the current foreground
            // target still matches the hotkey target; unknown focus is not
            // safe to paste.
            log.warn("recording focus was unavailable; automatic paste skipped");
            return false;
        }

        // Linux/X11 focus can be transiently unavailable. Preserve the existing
        // behavior there so a temporary X11 query failure does not drop speech.
        log.verbose("recording focus was unavailable; pasting without focus guard");
        return true;
    };

    focus_verification_allows_insertion(snapshot.verify_current(), focus.verification, log)
}

fn focus_verification_allows_insertion(
    result: Result<FocusVerification>,
    verification: &Cell<&'static str>,
    log: &Logger,
) -> bool {
    match result {
        Ok(result) => {
            verification.set(result.label());
            if result.allows_insertion() {
                true
            } else {
                log.warn("focus changed before insertion; automatic paste skipped");
                false
            }
        }
        Err(err) if cfg!(any(target_os = "macos", target_os = "windows")) => {
            verification.set("not_applicable");
            log.warn(format!(
                "could not verify recording focus ({err:#}); automatic paste skipped"
            ));
            false
        }
        Err(err) => {
            verification.set("not_applicable");
            log.verbose(format!(
                "could not verify recording focus ({err:#}); pasting without focus guard"
            ));
            true
        }
    }
}

/// Sanitizer decision for transcript insertion.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum PastePlan {
    /// Text may be pasted normally.
    Paste(String),
    /// Text may be copied, but should not be pasted automatically.
    CopyOnly {
        /// Sanitized text to copy.
        text: String,
        /// Why automatic paste was withheld.
        reason: PasteBlockReason,
    },
    /// Text should not be copied or pasted.
    Skip {
        /// Why the transcript was dropped.
        reason: PasteBlockReason,
    },
}

/// Why an utterance was not pasted automatically.
///
/// Carries two renderings of the same fact because the two consumers have
/// different audiences: [`Self::log_tag`] is a lowercase fragment that reads
/// correctly inside a parenthesized log line, and [`Self::notice`] is the
/// sentence shown in a desktop notification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PasteBlockReason {
    /// Nothing printable survived sanitization.
    EmptyAfterSanitization,
    /// Nothing printable survived terminal-mode newline trimming.
    EmptyTerminalAfterSanitization,
    /// Terminal mode refuses multi-line text, which would submit commands.
    MultilineTerminal,
    /// Longer than the terminal-mode paste ceiling.
    TerminalTooLong,
    /// Longer than the general paste ceiling.
    TooLong,
    /// The insertion circuit breaker is open after repeated failures.
    PasteTemporarilyDisabled,
    /// The platform insertion backend could not be prepared for this mode.
    BackendUnavailable,
    /// The focused target changed between capture and the insertion attempt.
    FocusChangedBeforeInsertion,
    /// The focused target changed during the final pre-chord recheck.
    FocusChangedBeforePaste,
    /// The paste chord was sent but never acknowledged, so the transcript was
    /// deliberately left on the clipboard for the user to paste manually.
    Unconfirmed,
}

impl PasteBlockReason {
    /// Lowercase fragment for log lines.
    ///
    /// # Returns
    ///
    /// A short phrase with no leading capital and no trailing period.
    fn log_tag(self) -> &'static str {
        match self {
            Self::EmptyAfterSanitization => "empty transcript after sanitization",
            Self::EmptyTerminalAfterSanitization => "empty terminal transcript after sanitization",
            Self::MultilineTerminal => "multiline terminal transcript",
            Self::TerminalTooLong => "terminal transcript too long",
            Self::TooLong => "transcript too long",
            Self::PasteTemporarilyDisabled => "paste temporarily disabled after repeated failures",
            Self::BackendUnavailable => "paste backend unavailable",
            Self::FocusChangedBeforeInsertion => "focus changed before insertion",
            Self::FocusChangedBeforePaste => "focus changed immediately before paste",
            Self::Unconfirmed => "paste not acknowledged",
        }
    }

    /// Sentence shown in the desktop notification body.
    ///
    /// # Returns
    ///
    /// A capitalized, punctuated sentence.
    fn notice(self) -> &'static str {
        match self {
            Self::EmptyAfterSanitization | Self::EmptyTerminalAfterSanitization => {
                "Transcript was empty after cleanup."
            }
            Self::MultilineTerminal => "Multi-line transcript; terminal mode does not auto-paste.",
            Self::TerminalTooLong | Self::TooLong => "Transcript too long to paste automatically.",
            Self::PasteTemporarilyDisabled => {
                "Paste is temporarily disabled after repeated failures."
            }
            Self::BackendUnavailable => "Paste backend was unavailable.",
            Self::FocusChangedBeforeInsertion => "Focus changed before insertion.",
            Self::FocusChangedBeforePaste => "Focus changed immediately before paste.",
            // The unacknowledged tier is only reachable where a platform
            // overrides `await_paste_confirmation` (macOS today), but name
            // the right chord for whatever platform this compiles for.
            #[cfg(target_os = "macos")]
            Self::Unconfirmed => {
                "Paste could not be confirmed; transcript copied. Press Cmd+V to insert it."
            }
            #[cfg(not(target_os = "macos"))]
            Self::Unconfirmed => {
                "Paste could not be confirmed; transcript copied. Press Ctrl+V to insert it."
            }
        }
    }
}

/// Result of a worker-level insertion attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InsertOutcome {
    Pasted,
    /// The paste chord was sent, but insertion could not be positively
    /// confirmed within the acknowledgement grace period. Treated as a
    /// success (the clipboard was restored per policy, same as
    /// [`Self::Pasted`]); distinct telemetry so this degraded case remains
    /// visible.
    PastedUnverified,
    CopiedOnly,
    Blocked,
    Skipped,
}

impl InsertOutcome {
    /// Return the stable label used for data-log insertion telemetry.
    ///
    /// # Returns
    ///
    /// A lowercase, `snake_case` outcome label.
    fn log_label(self) -> &'static str {
        match self {
            Self::Pasted => "pasted",
            Self::PastedUnverified => "pasted_unverified",
            Self::CopiedOnly => "copied_only",
            Self::Blocked => "blocked",
            Self::Skipped => "skipped",
        }
    }
}

/// Insertion outcome plus the real acknowledgement/clipboard telemetry
/// needed for [`log_insertion_outcome`], replacing the pre-acknowledgement
/// approximations (`paste_event_posted` inferred from the outcome label,
/// `clipboard_restored` always `None`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InsertReport {
    /// Coarse worker-level insertion result.
    pub(crate) outcome: InsertOutcome,
    /// Whether a synthetic paste chord or type event was actually sent.
    pub(crate) paste_event_posted: bool,
    /// How insertion success was acknowledged (`"ax_confirmed"`,
    /// `"unverified_timeout"`, `"no_evidence"`, or `"not_applicable"`).
    pub(crate) acknowledgement_kind: &'static str,
    /// Milliseconds spent waiting for acknowledgement, when applicable.
    pub(crate) acknowledgement_ms: Option<u128>,
    /// Whether the previous clipboard contents were restored, when the
    /// clipboard was touched at all.
    pub(crate) clipboard_restored: Option<bool>,
}

impl InsertReport {
    /// Build a report from a completed [`super::inject::PasteReport`],
    /// carrying its real acknowledgement/clipboard telemetry through.
    fn from_paste(outcome: InsertOutcome, report: super::inject::PasteReport) -> Self {
        Self {
            outcome,
            paste_event_posted: report.paste_event_posted,
            acknowledgement_kind: report.acknowledgement_kind,
            acknowledgement_ms: report.acknowledgement_ms,
            clipboard_restored: report.clipboard_restored,
        }
    }

    /// Build a report for a clipboard-staging-only path (no paste chord
    /// sent), with a known clipboard-restore outcome.
    fn from_stage(outcome: InsertOutcome, clipboard_restored: bool) -> Self {
        Self {
            outcome,
            paste_event_posted: false,
            acknowledgement_kind: "not_applicable",
            acknowledgement_ms: None,
            clipboard_restored: Some(clipboard_restored),
        }
    }

    /// Build a report for a path with no acknowledgement or clipboard
    /// telemetry at all (direct-mode insertion/block, or sanitizer skip).
    fn placeholder(outcome: InsertOutcome, paste_event_posted: bool) -> Self {
        Self {
            outcome,
            paste_event_posted,
            acknowledgement_kind: "not_applicable",
            acknowledgement_ms: None,
            clipboard_restored: None,
        }
    }
}

/// Sanitize text before any clipboard or paste action.
///
/// # Arguments
///
/// * `raw` - Candidate transcript or IPC text.
/// * `mode` - Paste mode, used for terminal-specific restrictions.
///
/// # Returns
///
/// A paste, copy, block, or skip decision.
pub(crate) fn sanitize_for_paste(raw: &str, mode: PasteMode) -> PastePlan {
    let normalized = raw.replace("\r\n", "\n").replace('\r', "\n");
    let mut text = String::with_capacity(normalized.len());
    for ch in normalized.chars() {
        match ch {
            '\0' => {}
            '\t' | '\n' => text.push(ch),
            ch if ch.is_ascii_control() => {}
            _ => text.push(ch),
        }
    }

    if text.trim().is_empty() {
        return PastePlan::Skip {
            reason: PasteBlockReason::EmptyAfterSanitization,
        };
    }

    if mode == PasteMode::Terminal {
        while text.ends_with('\n') {
            text.pop();
        }
        if text.trim().is_empty() {
            return PastePlan::Skip {
                reason: PasteBlockReason::EmptyTerminalAfterSanitization,
            };
        }
        if text.contains('\n') {
            return PastePlan::CopyOnly {
                text,
                reason: PasteBlockReason::MultilineTerminal,
            };
        }
        if text.chars().count() > TERMINAL_MAX_PASTE_CHARS {
            return PastePlan::CopyOnly {
                text,
                reason: PasteBlockReason::TerminalTooLong,
            };
        }
    }

    if text.chars().count() > MAX_PASTE_CHARS {
        return PastePlan::CopyOnly {
            text,
            reason: PasteBlockReason::TooLong,
        };
    }

    PastePlan::Paste(text)
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

fn capture_should_skip(pcm: &[f32]) -> bool {
    if pcm.is_empty() {
        return true;
    }

    let mut peak = 0.0_f32;
    let mut sum_squares = 0.0_f64;
    for sample in pcm {
        let abs = sample.abs();
        peak = peak.max(abs);
        sum_squares += f64::from(*sample) * f64::from(*sample);
    }
    let rms = (sum_squares / pcm.len() as f64).sqrt() as f32;
    peak < SILENCE_PEAK_THRESHOLD && rms < SILENCE_RMS_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::super::logging::LogLevel;
    use super::*;

    #[test]
    fn paste_circuit_reenables_after_cooldown() {
        let log = Logger::new(LogLevel::Quiet);
        let mut circuit = PasteCircuit::default();
        let start = Instant::now();

        assert!(!circuit.record_failure(start));
        assert!(!circuit.record_failure(start + Duration::from_millis(1)));
        let last_failure = start + Duration::from_millis(2);
        assert!(circuit.record_failure(last_failure));
        assert!(circuit.copy_only_mode);

        circuit.maybe_reenable(
            last_failure + PASTE_FAILURE_RESET_AFTER - Duration::from_millis(1),
            &log,
        );
        assert!(circuit.copy_only_mode);

        circuit.maybe_reenable(last_failure + PASTE_FAILURE_RESET_AFTER, &log);
        assert!(!circuit.copy_only_mode);
        assert_eq!(circuit.consecutive_failures, 0);
        assert!(circuit.last_failure.is_none());
    }

    #[test]
    fn paste_circuit_success_clears_failures() {
        let mut circuit = PasteCircuit::default();
        circuit.record_failure(Instant::now());

        circuit.record_success();

        assert!(!circuit.copy_only_mode);
        assert_eq!(circuit.consecutive_failures, 0);
        assert!(circuit.last_failure.is_none());
    }

    #[test]
    fn paste_circuit_expires_stale_partial_failures() {
        let mut circuit = PasteCircuit::default();
        let start = Instant::now();

        assert!(!circuit.record_failure(start));
        assert!(!circuit.record_failure(start + Duration::from_millis(1)));
        assert!(
            !circuit.record_failure(start + PASTE_FAILURE_RESET_AFTER + Duration::from_millis(1))
        );

        assert!(!circuit.copy_only_mode);
        assert_eq!(circuit.consecutive_failures, 1);
    }

    #[test]
    fn unavailable_or_unverified_focus_uses_platform_policy() {
        let log = Logger::new(LogLevel::Quiet);
        let verification = Cell::new("unset");

        assert!(focus_verification_allows_insertion(
            Ok(FocusVerification::Matched),
            &verification,
            &log
        ));
        assert_eq!(verification.get(), "matched");
        assert!(!focus_verification_allows_insertion(
            Ok(FocusVerification::Changed),
            &verification,
            &log
        ));
        assert_eq!(verification.get(), "changed");
        #[cfg(target_os = "macos")]
        {
            assert!(focus_verification_allows_insertion(
                Ok(FocusVerification::AxUnsupported),
                &verification,
                &log
            ));
            assert_eq!(verification.get(), "ax_unsupported");
        }

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        {
            let verification = Cell::new("not_applicable");
            let focus = FocusCheck {
                snapshot: None,
                verification: &verification,
            };
            assert!(!focus_allows_insertion(focus, &log));
            assert_eq!(verification.get(), "unavailable");
            assert!(!focus_verification_allows_insertion(
                Err(anyhow::anyhow!("focus unavailable")),
                &verification,
                &log
            ));
            assert_eq!(verification.get(), "not_applicable");
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let verification = Cell::new("not_applicable");
            let focus = FocusCheck {
                snapshot: None,
                verification: &verification,
            };
            assert!(focus_allows_insertion(focus, &log));
            assert_eq!(verification.get(), "unavailable");
            assert!(focus_verification_allows_insertion(
                Err(anyhow::anyhow!("temporary X11 failure")),
                &verification,
                &log
            ));
            assert_eq!(verification.get(), "not_applicable");
        }
    }

    #[test]
    fn paste_failures_use_clipboard_fallback_only_when_safe() {
        let paste_error = anyhow::anyhow!("could not send paste shortcut");
        assert!(paste_failure_uses_clipboard_fallback(
            PasteMode::Terminal,
            &paste_error,
            true
        ));
        assert!(!paste_failure_uses_clipboard_fallback(
            PasteMode::Terminal,
            &paste_error,
            false
        ));

        let restore_error =
            anyhow::anyhow!("{}: lost", super::super::inject::CLIPBOARD_RESTORE_ERROR);
        assert!(!paste_failure_uses_clipboard_fallback(
            PasteMode::Terminal,
            &restore_error,
            true
        ));

        let direct_error = anyhow::anyhow!("could not type text at cursor");
        assert!(!paste_failure_uses_clipboard_fallback(
            PasteMode::Direct,
            &direct_error,
            true
        ));
    }

    #[test]
    fn insertion_failures_remember_transcript_for_ipc_recovery() {
        fn report(outcome: InsertOutcome) -> InsertReport {
            InsertReport::placeholder(outcome, false)
        }

        assert!(insertion_result_remembers_transcript(&Ok(report(
            InsertOutcome::Pasted
        ))));
        assert!(insertion_result_remembers_transcript(&Ok(report(
            InsertOutcome::PastedUnverified
        ))));
        assert!(insertion_result_remembers_transcript(&Ok(report(
            InsertOutcome::CopiedOnly
        ))));
        assert!(insertion_result_remembers_transcript(&Ok(report(
            InsertOutcome::Blocked
        ))));
        assert!(insertion_result_remembers_transcript(&Err(
            anyhow::anyhow!("paste failed")
        )));
        assert!(!insertion_result_remembers_transcript(&Ok(report(
            InsertOutcome::Skipped
        ))));
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
    fn paste_sanitizer_cases_are_stable() {
        let raw = "a".repeat(TERMINAL_MAX_PASTE_CHARS + 1);
        let cases = [
            (
                "standard controls",
                "hello\0\r\nworld\x07".to_string(),
                PasteMode::Standard,
                PastePlan::Paste("hello\nworld".to_string()),
            ),
            (
                "terminal trailing newlines",
                "cargo test\n\n".to_string(),
                PasteMode::Terminal,
                PastePlan::Paste("cargo test".to_string()),
            ),
            (
                "terminal multiline copy-only",
                "first\nsecond".to_string(),
                PasteMode::Terminal,
                PastePlan::CopyOnly {
                    text: "first\nsecond".to_string(),
                    reason: PasteBlockReason::MultilineTerminal,
                },
            ),
            (
                "empty standard skip",
                "\0\x07\n".to_string(),
                PasteMode::Standard,
                PastePlan::Skip {
                    reason: PasteBlockReason::EmptyAfterSanitization,
                },
            ),
            (
                "long terminal copy-only",
                raw.clone(),
                PasteMode::Terminal,
                PastePlan::CopyOnly {
                    text: raw,
                    reason: PasteBlockReason::TerminalTooLong,
                },
            ),
        ];

        for (name, raw, mode, expected) in cases {
            assert_eq!(sanitize_for_paste(&raw, mode), expected, "{name}");
        }
    }
}
