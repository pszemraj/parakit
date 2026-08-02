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
use super::desktop::FocusVerification;
use super::inject::{ClipboardPolicy, FocusSnapshot, Injector, InsertionTelemetry, PasteMode};
use super::ipc::SharedState;
use super::logging::Logger;
use super::notifications::Notifier;
use super::sounds::Sounds;

/// Maximum number of worker events that may queue while ASR or paste is busy.
pub(crate) const WORKER_QUEUE_CAPACITY: usize = 2;

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
                if pcm.is_empty() {
                    log.verbose(format!(
                        "parakit: skipped empty capture ({secs:.2}s audio, {wall_secs:.2}s wall)"
                    ));
                    log.line("parakit: no speech detected");
                    state.set_phase("idle");
                    sounds.success();
                    continue;
                }
                state.set_phase("transcribing");
                log.transcribing(secs, wall_secs);

                let transcription = transcribe_clean(&engine, &pcm, cleaner.as_deref());
                // Inference is the last PCM consumer. Release the potentially
                // maximum-length capture before modifier-release and paste
                // acknowledgement waits keep the worker occupied.
                drop(pcm);

                match transcription {
                    Ok(Some(transcript)) => {
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
                        log.error(&format!("transcribe failed: {e:#}"));
                        state.set_phase("idle");
                        sounds.error();
                    }
                }
            }
        }
    }
}

fn prepared_injector(paste_mode: PasteMode) -> Result<Injector> {
    let mut injector = Injector::new()?;
    injector.prepare_for_mode(paste_mode)?;
    Ok(injector)
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
    let telemetry = report.map_or(InsertionTelemetry::not_applicable(false, None), |report| {
        report.telemetry
    });
    data_log.log_insertion(
        &record_id,
        InsertionLogFields {
            outcome,
            target_bundle_id,
            focus_verification,
            transcript_chars,
            paste_event_posted: telemetry.paste_event_posted,
            // NSPasteboard/Carbon promise-keeper read evidence is a
            // deferred follow-up (see `daemon::macos::pasteboard` module
            // docs); no call site can populate this yet.
            pasteboard_requested: None,
            acknowledgement_kind: telemetry.acknowledgement_kind,
            acknowledgement_ms: telemetry.acknowledgement_ms,
            clipboard_restored: telemetry.clipboard_restored,
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
) -> Result<InsertReport> {
    let (log, notifier) = ui;
    match sanitize_for_paste(raw_text, mode) {
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

    let prepare_result =
        ensure_injector(injector, "could not initialize insertion backend")?.prepare_for_mode(mode);
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
                warn_if_clipboard_restore_failed(log, keep_transcript_clipboard, &report);
                return Ok(InsertReport::from_paste(InsertOutcome::Pasted, report));
            }
            super::inject::PasteOutcome::PastedUnverified => {
                warn_if_clipboard_restore_failed(log, keep_transcript_clipboard, &report);
                log.verbose(format!(
                    "parakit: paste sent but insertion could not be confirmed within {}ms ({}); treating as pasted",
                    report.telemetry.acknowledgement_ms.unwrap_or_default(),
                    report.telemetry.acknowledgement_kind,
                ));
                return Ok(InsertReport::from_paste(
                    InsertOutcome::PastedUnverified,
                    report,
                ));
            }
            super::inject::PasteOutcome::CopiedOnly => {
                if report.telemetry.paste_event_posted {
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
            super::inject::PasteOutcome::UnsafeModifiers => {
                // No chord was ever posted (`report.telemetry.paste_event_posted` is
                // always false here) and the transcript is intentionally
                // kept on the clipboard, exactly like the pre-chord
                // `CopiedOnly` case above: this is the quiet "copied, not
                // pasted" outcome, not a `Blocked` failure.
                notifier.transcript_copied(PasteBlockReason::UnsafeModifiers.notice());
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
        with_injector(injector, |injector| injector.copy_text(text)).map_err(|copy_error| {
            anyhow::anyhow!("{paste_error:#}; clipboard fallback also failed: {copy_error:#}")
        })?;
        return Err(
            paste_error.context("transcript copied to clipboard fallback after paste failure")
        );
    }

    Err(paste_error)
}

/// Warn when a paste that already landed (or was accepted as unverified)
/// could not restore the caller's previous clipboard contents.
///
/// `Injector::paste_text_guarded` never turns a failed restore into an error
/// once the paste itself reached the `Pasted`/`PastedUnverified` tier: the
/// paste already happened, so losing the previous clipboard is a secondary,
/// recoverable problem, not a paste failure. That fix means this is the only
/// place the failure becomes visible, since the low-level insertion module
/// has no logger of its own to report through.
///
/// `report.telemetry.clipboard_restored == Some(false)` is ambiguous on its own: it is
/// also what a deliberate [`ClipboardPolicy::KeepTranscript`] request looks
/// like. Restricting the warning to `!keep_transcript_clipboard` (the caller
/// asked for [`ClipboardPolicy::RestorePrevious`]) resolves that half of the
/// ambiguity, since that combination usually can only mean the restore was
/// attempted and failed.
///
/// The remaining exception is `acknowledgement_kind: "unverified_focus_lost"`
/// (see [`crate::daemon::desktop::clipboard_restore::PasteConfirmation::UnverifiedFocusLost`]):
/// there, `finish_confirmed_paste` never even attempts a restore, because the
/// insertion target became unobservable before one could be trusted, and
/// deliberately keeps the transcript for the same reason the `CopiedOnly`
/// no-evidence path does. Warning about a "failed" restore that was never
/// attempted would misreport a deliberate, correct decision as a clipboard
/// bug, so that kind is excluded here exactly as `CopiedOnly` already is by
/// this function never being called for that outcome.
fn warn_if_clipboard_restore_failed(
    log: &Logger,
    keep_transcript_clipboard: bool,
    report: &super::inject::PasteReport,
) {
    if !keep_transcript_clipboard
        && report.telemetry.clipboard_restored == Some(false)
        && report.telemetry.acknowledgement_kind != "unverified_focus_lost"
    {
        log.warn(format!(
            "paste succeeded, but {}; the transcript is likely still on the clipboard",
            super::inject::CLIPBOARD_RESTORE_ERROR
        ));
    }
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
    let policy = clipboard_policy(keep_transcript_clipboard);
    match with_injector(injector, |injector| {
        injector.stage_text_for_history(text, policy)
    })
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

fn with_injector<R>(
    injector: &mut Option<Injector>,
    f: impl FnOnce(&mut Injector) -> Result<R>,
) -> Result<R> {
    f(ensure_injector(
        injector,
        "could not initialize insertion backend for clipboard fallback",
    )?)
}

fn ensure_injector<'a>(
    injector: &'a mut Option<Injector>,
    context: &'static str,
) -> Result<&'a mut Injector> {
    if injector.is_none() {
        *injector = Some(Injector::new().context(context)?);
    }
    Ok(injector
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
            match result {
                FocusVerification::Matched => true,
                #[cfg(target_os = "macos")]
                FocusVerification::AxUnsupported => {
                    log.verbose(
                        "macOS Accessibility did not expose focused-element identity; using the matching pid and bundle-id focus fallback",
                    );
                    true
                }
                FocusVerification::Changed => {
                    log.warn("focus changed before insertion; automatic paste skipped");
                    false
                }
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
    /// Terminal mode refuses multi-line text, which would submit commands.
    MultilineTerminal,
    /// The platform insertion backend could not be prepared for this mode.
    BackendUnavailable,
    /// The focused target changed between capture and the insertion attempt.
    FocusChangedBeforeInsertion,
    /// The focused target changed during the final pre-chord recheck.
    FocusChangedBeforePaste,
    /// Physical modifier keys remained active, so posting the paste shortcut
    /// would have produced a different chord.
    UnsafeModifiers,
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
            Self::MultilineTerminal => "multiline terminal transcript",
            Self::BackendUnavailable => "paste backend unavailable",
            Self::FocusChangedBeforeInsertion => "focus changed before insertion",
            Self::FocusChangedBeforePaste => "focus changed immediately before paste",
            Self::UnsafeModifiers => "physical modifiers made the paste shortcut unsafe",
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
            Self::EmptyAfterSanitization => "Transcript was empty after cleanup.",
            Self::MultilineTerminal => "Multi-line transcript; terminal mode does not auto-paste.",
            Self::BackendUnavailable => "Paste backend was unavailable.",
            Self::FocusChangedBeforeInsertion => "Focus changed before insertion.",
            Self::FocusChangedBeforePaste => "Focus changed immediately before paste.",
            Self::UnsafeModifiers => {
                "Push-to-talk keys remained held; transcript copied for manual paste."
            }
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
    /// Acknowledgement and clipboard details from the insertion path.
    pub(crate) telemetry: InsertionTelemetry,
}

impl InsertReport {
    /// Build a report from a completed [`super::inject::PasteReport`],
    /// carrying its real acknowledgement/clipboard telemetry through.
    fn from_paste(outcome: InsertOutcome, report: super::inject::PasteReport) -> Self {
        Self {
            outcome,
            telemetry: report.telemetry,
        }
    }

    /// Build a report for a clipboard-staging-only path (no paste chord
    /// sent), with a known clipboard-restore outcome.
    fn from_stage(outcome: InsertOutcome, clipboard_restored: bool) -> Self {
        Self {
            outcome,
            telemetry: InsertionTelemetry::not_applicable(false, Some(clipboard_restored)),
        }
    }

    /// Build a report for a path with no acknowledgement or clipboard
    /// telemetry at all (direct-mode insertion/block, or sanitizer skip).
    fn placeholder(outcome: InsertOutcome, paste_event_posted: bool) -> Self {
        Self {
            outcome,
            telemetry: InsertionTelemetry::not_applicable(paste_event_posted, None),
        }
    }

    /// Whether this outcome should play the daemon's error tone instead of
    /// its success tone.
    ///
    /// [`InsertOutcome::Blocked`] always alarms. A post-chord
    /// [`InsertOutcome::CopiedOnly`] (a paste chord was actually sent but
    /// insertion was never confirmed) is a safe degradation, not a backend
    /// failure, but the user still needs to know the paste did not land; a
    /// pre-chord `CopiedOnly` (no chord was ever sent — guard-blocked, focus
    /// changed before paste, or held modifiers withheld the chord) keeps the
    /// quieter existing behavior.
    ///
    /// # Returns
    ///
    /// `true` when the daemon should play its error tone for this outcome.
    fn needs_alert(&self) -> bool {
        matches!(self.outcome, InsertOutcome::Blocked)
            || (self.outcome == InsertOutcome::CopiedOnly && self.telemetry.paste_event_posted)
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
        if text.contains('\n') {
            return PastePlan::CopyOnly {
                text,
                reason: PasteBlockReason::MultilineTerminal,
            };
        }
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

#[cfg(test)]
mod tests {
    use super::super::logging::LogLevel;
    use super::*;

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

        /// Expected `insertion_result_remembers_transcript` result for
        /// `outcome`.
        ///
        /// Exhaustive with no wildcard arm: a new `InsertOutcome` variant
        /// fails compilation here until this function states its
        /// expectation.
        fn expected_remembers_transcript(outcome: InsertOutcome) -> bool {
            match outcome {
                InsertOutcome::Pasted
                | InsertOutcome::PastedUnverified
                | InsertOutcome::CopiedOnly
                | InsertOutcome::Blocked => true,
                InsertOutcome::Skipped => false,
            }
        }

        let outcomes = [
            InsertOutcome::Pasted,
            InsertOutcome::PastedUnverified,
            InsertOutcome::CopiedOnly,
            InsertOutcome::Blocked,
            InsertOutcome::Skipped,
        ];

        let failures: Vec<String> = outcomes
            .iter()
            .filter_map(|&outcome| {
                let expected = expected_remembers_transcript(outcome);
                let actual = insertion_result_remembers_transcript(&Ok(report(outcome)));
                (actual != expected)
                    .then(|| format!("{outcome:?}: expected {expected}, got {actual}"))
            })
            .collect();
        assert!(
            failures.is_empty(),
            "{} case(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );

        assert!(insertion_result_remembers_transcript(&Err(
            anyhow::anyhow!("paste failed")
        )));
    }

    #[test]
    fn paste_sanitizer_cases_are_stable() {
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
        ];

        for (name, raw, mode, expected) in cases {
            assert_eq!(sanitize_for_paste(&raw, mode), expected, "{name}");
        }
    }

    #[test]
    fn needs_alert_matches_outcome_and_paste_event_posted() {
        fn report(outcome: InsertOutcome, paste_event_posted: bool) -> InsertReport {
            InsertReport {
                outcome,
                telemetry: InsertionTelemetry::not_applicable(paste_event_posted, None),
            }
        }

        /// Expected `needs_alert()` results for `outcome`, as
        /// `(paste_event_posted, expected)` pairs.
        ///
        /// Exhaustive with no wildcard arm: a new `InsertOutcome` variant
        /// fails compilation here until this function states its alert
        /// expectation.
        fn needs_alert_cases(outcome: InsertOutcome) -> &'static [(bool, bool)] {
            match outcome {
                InsertOutcome::Pasted => &[(true, false)],
                InsertOutcome::PastedUnverified => &[(true, false)],
                InsertOutcome::Skipped => &[(false, false)],
                InsertOutcome::Blocked => &[(false, true), (true, true)],
                InsertOutcome::CopiedOnly => &[
                    // pre-chord CopiedOnly (guard-blocked, focus changed, or
                    // unsafe modifiers) stays quiet
                    (false, false),
                    // post-chord CopiedOnly (chord sent but never confirmed)
                    // must still alert
                    (true, true),
                ],
            }
        }

        let outcomes = [
            InsertOutcome::Pasted,
            InsertOutcome::PastedUnverified,
            InsertOutcome::CopiedOnly,
            InsertOutcome::Blocked,
            InsertOutcome::Skipped,
        ];

        let failures: Vec<String> = outcomes
            .iter()
            .flat_map(|&outcome| {
                needs_alert_cases(outcome)
                    .iter()
                    .filter_map(move |&(paste_event_posted, expected)| {
                        let actual = report(outcome, paste_event_posted).needs_alert();
                        (actual != expected).then(|| {
                            format!(
                                "{outcome:?} paste_event_posted={paste_event_posted}: expected {expected}, got {actual}"
                            )
                        })
                    })
            })
            .collect();

        assert!(
            failures.is_empty(),
            "{} case(s) failed:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }
}
