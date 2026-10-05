//! Existing paste safety boundary, shared by dictation and IPC insertion.

use crate::daemon::desktop::FocusVerification;
use crate::daemon::inject::{
    ClipboardPolicy, FocusSnapshot, Injector, InsertionTelemetry, PasteMode,
};
use crate::daemon::logging::Logger;
use crate::daemon::notifications::Notifier;
use anyhow::{Context, Result};
use parakit::data_log::{DataLogger, InsertionLogFields, RecordId};
use std::cell::Cell;
use std::sync::Arc;

/// Initialize the insertion backend for the worker's configured paste mode.
///
/// # Returns
///
/// An injector ready for insertion.
///
/// # Errors
///
/// Reports unavailable platform insertion facilities.
pub(super) fn prepared_injector(paste_mode: PasteMode) -> Result<Injector> {
    let mut injector = Injector::new()?;
    injector.prepare_for_mode(paste_mode)?;
    Ok(injector)
}

/// Retain transcripts for recovery unless sanitization intentionally skipped them.
///
/// # Returns
///
/// Whether the outcome belongs in the in-memory transcript history.
pub(super) fn insertion_result_remembers_transcript(result: &Result<InsertReport>) -> bool {
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
pub(super) fn log_insertion_outcome(
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
            || {
                if mode == PasteMode::Direct {
                    focus_allows_direct_insertion(focus, log)
                } else {
                    Ok(focus_allows_insertion(focus, log))
                }
            },
        );
    let paste_error = match paste_result {
        Ok(report) => match report.outcome {
            crate::daemon::inject::PasteOutcome::Pasted => {
                warn_if_clipboard_restore_failed(log, keep_transcript_clipboard, &report);
                return Ok(InsertReport::from_paste(InsertOutcome::Pasted, report));
            }
            crate::daemon::inject::PasteOutcome::PastedUnverified => {
                warn_if_clipboard_restore_failed(log, keep_transcript_clipboard, &report);
                log.verbose(format!(
                    "parakit: paste sent but insertion could not be confirmed within {}ms ({}); treating as pasted",
                    report.telemetry.acknowledgement_ms.unwrap_or_default(),
                    report.telemetry.acknowledgement_kind,
                ));
                if report.telemetry.acknowledgement_kind == "no_evidence" {
                    notifier.paste_blocked(
                        "Paste could not be confirmed and the clipboard changed. Inspect the target before retrying; the full transcript remains in history.",
                    );
                }
                return Ok(InsertReport::from_paste(
                    InsertOutcome::PastedUnverified,
                    report,
                ));
            }
            crate::daemon::inject::PasteOutcome::CopiedOnly => {
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
            crate::daemon::inject::PasteOutcome::UnsafeModifiers => {
                // No chord was ever posted (`report.telemetry.paste_event_posted` is
                // always false here) and the transcript is intentionally
                // kept on the clipboard, exactly like the pre-chord
                // `CopiedOnly` case above: this is the quiet "copied, not
                // pasted" outcome, not a `Blocked` failure.
                notifier.transcript_copied(PasteBlockReason::UnsafeModifiers.notice());
                return Ok(InsertReport::from_paste(InsertOutcome::CopiedOnly, report));
            }
            crate::daemon::inject::PasteOutcome::Blocked => {
                notifier.paste_blocked(PasteBlockReason::FocusChangedBeforePaste.notice());
                return Ok(InsertReport::from_paste(InsertOutcome::Blocked, report));
            }
            crate::daemon::inject::PasteOutcome::ClipboardChanged => {
                log.warn("clipboard changed or could not be verified; current clipboard preserved");
                notifier.paste_blocked(PasteBlockReason::ClipboardChanged.notice());
                return Ok(InsertReport::from_paste(InsertOutcome::Blocked, report));
            }
        },
        Err(err) => err,
    };

    if let Some(failure) = paste_error.downcast_ref::<crate::daemon::inject::DirectTypingFailure>()
    {
        let reason = failure.reason();
        log.warn(format!(
            "direct insertion stopped after {} of {} characters: {reason}",
            failure.typed_chars(),
            failure.total_chars()
        ));
        notifier.paste_blocked(format!(
            "Direct typing stopped after {} of {} characters. Check the target before using copy-last; the full transcript remains in history.",
            failure.typed_chars(),
            failure.total_chars()
        ));
        return Ok(InsertReport::direct_failure(
            failure.typed_chars(),
            format!(
                "direct typing stopped after {} of {} characters: {reason}",
                failure.typed_chars(),
                failure.total_chars()
            ),
        ));
    }

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
    report: &crate::daemon::inject::PasteReport,
) {
    if report.telemetry.acknowledgement_ms.is_some()
        && report.telemetry.clipboard_restored.is_none()
    {
        log.verbose("parakit: clipboard restoration skipped because contents changed or could not be verified; current clipboard preserved");
    }
    if !keep_transcript_clipboard
        && report.telemetry.clipboard_restored == Some(false)
        && report.telemetry.acknowledgement_kind != "unverified_focus_lost"
    {
        log.warn(format!(
            "paste succeeded, but {}; the transcript is likely still on the clipboard",
            crate::daemon::inject::CLIPBOARD_RESTORE_ERROR
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
        crate::daemon::inject::StageOutcome::CopiedOnly => {
            notifier.transcript_copied(reason.notice());
            Ok(InsertReport::from_stage(InsertOutcome::CopiedOnly, false))
        }
        crate::daemon::inject::StageOutcome::Blocked => {
            log.warn(format!(
                "automatic paste skipped ({}); transcript staged for clipboard history",
                reason.log_tag()
            ));
            notifier.paste_blocked(reason.notice());
            Ok(InsertReport::from_stage(InsertOutcome::Blocked, true))
        }
        crate::daemon::inject::StageOutcome::ClipboardChanged => {
            log.warn("clipboard changed or could not be verified; current clipboard preserved");
            notifier.paste_blocked(PasteBlockReason::ClipboardChanged.notice());
            Ok(InsertReport::placeholder(InsertOutcome::Blocked, false))
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
        && !message.contains(crate::daemon::inject::CLIPBOARD_RESTORE_ERROR)
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

/// Recheck Linux direct-mode focus without treating an observation failure as
/// permission to continue posting characters.
fn focus_allows_direct_insertion(focus: FocusCheck<'_>, log: &Logger) -> Result<bool> {
    let Some(snapshot) = focus.snapshot else {
        focus.verification.set("unavailable");
        log.verbose("recording focus was unavailable; direct typing has no focus baseline");
        return Ok(true);
    };

    let verification = snapshot.verify_current().map_err(|err| {
        focus.verification.set("not_applicable");
        err.context("could not verify recording focus during direct typing")
    })?;
    Ok(focus_verification_allows_insertion(
        Ok(verification),
        focus.verification,
        log,
    ))
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
    /// Clipboard contents changed or could not be verified during insertion.
    ClipboardChanged,
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
            Self::ClipboardChanged => "clipboard changed or unavailable",
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
            Self::ClipboardChanged => {
                "Clipboard changed or was unavailable; automatic insertion stopped."
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
    pub(super) fn log_label(self) -> &'static str {
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InsertReport {
    /// Coarse worker-level insertion result.
    pub(crate) outcome: InsertOutcome,
    /// Acknowledgement and clipboard details from the insertion path.
    pub(crate) telemetry: InsertionTelemetry,
    /// Characters whose direct-typing backend calls completed before a block.
    pub(crate) typed_chars: Option<usize>,
    /// Diagnostic for a completed but degraded insertion outcome.
    pub(crate) failure_reason: Option<String>,
}

impl InsertReport {
    /// Build a report from a completed [`crate::daemon::inject::PasteReport`],
    /// carrying its real acknowledgement/clipboard telemetry through.
    fn from_paste(outcome: InsertOutcome, report: crate::daemon::inject::PasteReport) -> Self {
        Self {
            outcome,
            telemetry: report.telemetry,
            typed_chars: None,
            failure_reason: None,
        }
    }

    /// Build a report for a clipboard-staging-only path (no paste chord
    /// sent), with a known clipboard-restore outcome.
    fn from_stage(outcome: InsertOutcome, clipboard_restored: bool) -> Self {
        Self {
            outcome,
            telemetry: InsertionTelemetry::not_applicable(false, Some(clipboard_restored)),
            typed_chars: None,
            failure_reason: None,
        }
    }

    /// Build a report for a path with no acknowledgement or clipboard
    /// telemetry at all (direct-mode insertion/block, or sanitizer skip).
    fn placeholder(outcome: InsertOutcome, paste_event_posted: bool) -> Self {
        Self {
            outcome,
            telemetry: InsertionTelemetry::not_applicable(paste_event_posted, None),
            typed_chars: None,
            failure_reason: None,
        }
    }

    fn direct_failure(typed_chars: usize, failure_reason: String) -> Self {
        Self {
            outcome: InsertOutcome::Blocked,
            telemetry: InsertionTelemetry::not_applicable(typed_chars > 0, None),
            typed_chars: Some(typed_chars),
            failure_reason: Some(failure_reason),
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
    pub(super) fn needs_alert(&self) -> bool {
        matches!(self.outcome, InsertOutcome::Blocked)
            || (self.outcome == InsertOutcome::CopiedOnly && self.telemetry.paste_event_posted)
            || (self.outcome == InsertOutcome::PastedUnverified
                && self.telemetry.acknowledgement_kind == "no_evidence")
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

    // Length alone is not unsafe: recording duration and IPC framing are
    // bounded at their entry points, and long single-line dictation remains valid.
    PastePlan::Paste(text)
}

#[cfg(test)]
#[path = "insertion_tests.rs"]
mod tests;
