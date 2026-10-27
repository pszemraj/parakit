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
use std::fmt;
use std::sync::Arc;

const REQUIRE_VERIFIED_FOCUS: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// Initialize the insertion backend for the worker's configured paste mode.
///
/// # Returns
/// An injector ready for insertion.
///
/// # Errors
/// Reports unavailable platform insertion facilities.
pub(super) fn prepared_injector(paste_mode: PasteMode) -> Result<Injector> {
    let mut injector = Injector::new()?;
    injector.prepare_for_mode(paste_mode)?;
    Ok(injector)
}

/// Retain transcripts for recovery unless sanitization intentionally skipped them.
///
/// # Returns
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

/// Return delivery evidence retained by an insertion or clipboard-recovery failure.
///
/// # Returns
/// The retained native/direct report, otherwise `None`.
pub(super) fn insertion_error_report(error: &anyhow::Error) -> Option<&InsertReport> {
    error
        .downcast_ref::<ReportedInsertionError>()
        .map(|error| &error.report)
}

/// Write insertion telemetry for a logged transcription; absent logger/id is a no-op.
///
/// # Arguments
/// * `data_log` - Optional shared transcription logger.
/// * `record_id` - Transcription record to correlate with.
/// * `outcome` - Coarse insertion result label.
/// * `focus_at_start` - Original target, including its bundle ID when available.
/// * `transcript_chars` - Character count offered for insertion.
/// * `failure_reason` - Error or degraded-outcome diagnostic text.
/// * `focus_verification` - Last live recheck label, or `"not_applicable"` without a check.
/// * `report` - Observed acknowledgement/clipboard details, absent before observable input.
/// * `paste_mode` - Content policy and legacy direct-typing selection.
#[allow(
    clippy::too_many_arguments,
    reason = "insertion telemetry combines independently observed facts"
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
    paste_mode: PasteMode,
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
            insertion_backend: report.map(|_| {
                #[cfg(target_os = "linux")]
                if focus_at_start.is_some_and(FocusSnapshot::is_native) {
                    return "ibus";
                }
                if paste_mode == PasteMode::Direct {
                    "direct"
                } else {
                    "clipboard"
                }
            }),
            outcome,
            target_bundle_id,
            focus_verification,
            transcript_chars,
            paste_event_posted: telemetry.paste_event_posted,
            // Clipboard-read evidence is unavailable from the current backends.
            pasteboard_requested: None,
            acknowledgement_kind: telemetry.acknowledgement_kind,
            acknowledgement_ms: telemetry.acknowledgement_ms,
            clipboard_restored: telemetry.clipboard_restored,
            failure_reason: failure_reason.as_deref(),
        },
    );
}

/// Original focus snapshot and the telemetry label of its last live recheck.
#[derive(Clone, Copy)]
pub(crate) struct FocusCheck<'a> {
    /// Focus captured at PTT-down, when capture succeeded.
    pub(crate) snapshot: Option<&'a FocusSnapshot>,
    /// Last live recheck label; unchanged when insertion needs no focus check.
    pub(crate) verification: &'a Cell<&'static str>,
}

/// Sanitize text and run the shared paste/copy insertion transaction.
///
/// # Arguments
/// * `injector` - Reused insertion backend, created lazily when needed.
/// * `raw_text` - Candidate transcript or IPC text.
/// * `mode` - Paste mode used for sanitizer policy and chord selection.
/// * `keep_transcript_clipboard` - Leave text on clipboard instead of restoring previous contents.
/// * `focus` - Focus snapshot and telemetry cell (see [`FocusCheck`]).
/// * `ui` - Daemon logger and desktop notification wrapper.
///
/// # Returns
/// Worker insertion outcome plus acknowledgement/clipboard telemetry.
///
/// # Errors
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
    #[cfg(target_os = "linux")]
    if focus.snapshot.is_some_and(FocusSnapshot::is_native) {
        return insert_native(injector, raw_text, mode, focus, (log, notifier));
    }
    log.verbose(format!(
        "parakit: insertion transport={}",
        if mode == PasteMode::Direct {
            "direct"
        } else {
            "clipboard"
        }
    ));
    let result = match sanitize_for_paste(raw_text, mode) {
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
        PastePlan::Skip { reason } => {
            log.verbose(format!("paste skipped by sanitizer: {}", reason.log_tag()));
            Ok(InsertReport::placeholder(InsertOutcome::Skipped, false))
        }
    };
    if let Err(error) = &result {
        let typed_chars = insertion_error_report(error).and_then(|report| report.typed_chars);
        notifier.insertion_failed(typed_chars);
    }
    result
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
        Ok(report) => {
            return Ok(finish_paste(
                report,
                keep_transcript_clipboard,
                log,
                notifier,
            ))
        }
        Err(err) => err,
    };

    #[cfg(target_os = "linux")]
    if let Some(failure) = paste_error.downcast_ref::<crate::daemon::inject::DirectTypingFailure>()
    {
        if !failure.is_blocked() {
            let failure_reason = format!(
                "direct typing failed after {} of {} characters: {}",
                failure.typed_chars(),
                failure.total_chars(),
                failure.reason()
            );
            log.warn(&failure_reason);
            return Err(ReportedInsertionError {
                report: InsertReport::direct_failure(failure.typed_chars(), failure_reason.clone()),
                message: failure_reason,
            }
            .into());
        }
        let reason = failure.reason();
        log.warn(format!(
            "direct insertion stopped after {} of {} characters: {reason}",
            failure.typed_chars(),
            failure.total_chars()
        ));
        notifier.paste_blocked(format!(
            "Direct typing stopped after {} of {} characters. Check the target before retrying; dictations remain available through copy-last when history is enabled.",
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

/// Apply content policy and submit once to the recording's native target.
#[cfg(target_os = "linux")]
fn insert_native(
    injector: &mut Option<Injector>,
    raw_text: &str,
    mode: PasteMode,
    focus: FocusCheck<'_>,
    ui: (&Logger, &Notifier),
) -> Result<InsertReport> {
    use crate::daemon::desktop::ibus::CommitOutcome;
    let (log, notifier) = ui;
    let plan = sanitize_for_paste(raw_text, mode);
    let text = match plan {
        PastePlan::Paste(text) | PastePlan::CopyOnly { text, .. } => text,
        PastePlan::Skip { .. } => {
            return Ok(InsertReport::placeholder(InsertOutcome::Skipped, false));
        }
    };
    let terminal_blocked =
        mode == PasteMode::Terminal && (text.contains('\t') || text.contains('\n'));
    let (outcome, kind, reason, recovery_reason) = if terminal_blocked {
        let reason = "terminal content policy blocked native tabs or embedded newlines".to_owned();
        log.warn(&reason);
        (
            InsertOutcome::Blocked,
            "not_applicable",
            reason,
            PasteBlockReason::NativeTerminalControls,
        )
    } else {
        match focus
            .snapshot
            .expect("native insertion has a captured route")
            .commit_native(&text)
        {
            CommitOutcome::Submitted => {
                focus.verification.set("matched");
                log.verbose(
                "parakit: transcript submitted through IBus; application reception is unconfirmed",
            );
                let mut report = InsertReport::placeholder(InsertOutcome::Submitted, false);
                report.telemetry.acknowledgement_kind = "ibus_submitted";
                return Ok(report);
            }
            CommitOutcome::Rejected(reason) => {
                // Rejection does not distinguish target drift from connection loss.
                focus.verification.set("not_applicable");
                log.warn(format!("IBus insertion rejected: {reason}"));
                (
                    InsertOutcome::Blocked,
                    "ibus_rejected",
                    reason,
                    PasteBlockReason::NativeRejected,
                )
            }
            CommitOutcome::Uncertain(reason) => {
                focus.verification.set("not_applicable");
                log.warn(format!(
                    "IBus delivery is uncertain: {reason}; automatic insertion will not be retried"
                ));
                (
                    InsertOutcome::Uncertain,
                    "ibus_uncertain",
                    reason,
                    PasteBlockReason::NativeUncertain,
                )
            }
        }
    };
    let mut report = InsertReport::placeholder(outcome, false);
    report.telemetry.acknowledgement_kind = kind;
    report.failure_reason = Some(reason);
    let recovery = copy_or_block_transcript(
        injector,
        &text,
        // Failed native delivery must remain recoverable even with history
        // disabled; the successful-paste clipboard preference does not apply.
        true,
        "native transcript clipboard recovery failed",
        recovery_reason,
        log,
        notifier,
    );
    finish_native_recovery(report, recovery)
}

/// Retain native delivery evidence while recording clipboard recovery separately.
#[cfg(target_os = "linux")]
fn finish_native_recovery(
    mut native: InsertReport,
    recovery: Result<InsertReport>,
) -> Result<InsertReport> {
    match recovery {
        Ok(recovery) => {
            if native.outcome != InsertOutcome::Uncertain {
                native.outcome = recovery.outcome;
            }
            native.telemetry.clipboard_restored = recovery.telemetry.clipboard_restored;
            if let Some(diagnostic) = recovery.failure_reason {
                let reason = native.failure_reason.get_or_insert_with(String::new);
                reason.push_str("; ");
                reason.push_str(&diagnostic);
            }
            Ok(native)
        }
        Err(error) => Err(ReportedInsertionError {
            message: format!(
                "{}; clipboard recovery failed: {error:#}",
                native
                    .failure_reason
                    .as_deref()
                    .unwrap_or("native insertion failed")
            ),
            report: native,
        }
        .into()),
    }
}

/// Report the actual paste outcome through the worker's existing UI channels.
fn finish_paste(
    report: crate::daemon::inject::PasteReport,
    keep_transcript_clipboard: bool,
    log: &Logger,
    notifier: &Notifier,
) -> InsertReport {
    use crate::daemon::inject::PasteOutcome;

    let outcome = match report.outcome {
        PasteOutcome::Pasted => {
            log_clipboard_restore_diagnostic(log, keep_transcript_clipboard, &report);
            InsertOutcome::Pasted
        }
        PasteOutcome::PastedUnverified => {
            log_clipboard_restore_diagnostic(log, keep_transcript_clipboard, &report);
            log.verbose(format!(
                "parakit: paste sent but insertion could not be confirmed within {}ms ({}); treating as pasted",
                report.telemetry.acknowledgement_ms.unwrap_or_default(),
                report.telemetry.acknowledgement_kind,
            ));
            if report.telemetry.acknowledgement_kind == "no_evidence" {
                notifier.paste_unconfirmed(PasteBlockReason::UnconfirmedClipboardChanged.notice());
            }
            InsertOutcome::PastedUnverified
        }
        PasteOutcome::CopiedOnly => {
            if report.telemetry.paste_event_posted {
                notifier.paste_unconfirmed(PasteBlockReason::Unconfirmed.notice());
            } else {
                notifier.transcript_copied(PasteBlockReason::FocusChangedBeforePaste.notice());
            }
            InsertOutcome::CopiedOnly
        }
        PasteOutcome::UnsafeModifiers => {
            log.warn(PasteBlockReason::UnsafeModifiers.notice());
            notifier.transcript_copied(PasteBlockReason::UnsafeModifiers.notice());
            // No chord was posted; preserve the quieter copy-only tone.
            InsertOutcome::CopiedOnly
        }
        PasteOutcome::Blocked => {
            notifier.paste_blocked(PasteBlockReason::FocusChangedBeforePaste.notice());
            InsertOutcome::Blocked
        }
        PasteOutcome::ClipboardChanged => {
            log_clipboard_changed(log, report.diagnostic.as_deref());
            notifier.paste_blocked(PasteBlockReason::ClipboardChanged.notice());
            InsertOutcome::Blocked
        }
    };
    InsertReport::from_paste(outcome, report)
}

fn log_clipboard_changed(log: &Logger, diagnostic: Option<&str>) {
    if let Some(diagnostic) = diagnostic {
        log.warn(format!(
            "clipboard could not be verified ({diagnostic}); current clipboard preserved"
        ));
    } else {
        log.warn("clipboard changed; current clipboard preserved");
    }
}

/// Log secondary clipboard details after delivery, retaining primary diagnostics in telemetry.
fn log_clipboard_restore_diagnostic(
    log: &Logger,
    keep_transcript_clipboard: bool,
    report: &crate::daemon::inject::PasteReport,
) {
    if let Some(diagnostic) = clipboard_restore_diagnostic(keep_transcript_clipboard, report) {
        log.verbose(diagnostic);
        return;
    }
    if report.telemetry.acknowledgement_ms.is_some()
        && report.telemetry.clipboard_restored.is_none()
        && report.diagnostic.is_none()
    {
        log.verbose("parakit: clipboard restoration skipped because contents changed or could not be verified; current clipboard preserved");
    }
    if let Some(diagnostic) = report.diagnostic.as_deref() {
        log.verbose(format!(
            "clipboard could not be verified after paste ({diagnostic}); current clipboard preserved"
        ));
    }
}

fn clipboard_restore_diagnostic(
    keep_transcript_clipboard: bool,
    report: &crate::daemon::inject::PasteReport,
) -> Option<String> {
    if keep_transcript_clipboard
        || report.telemetry.clipboard_restored != Some(false)
        || report.telemetry.acknowledgement_kind == "unverified_focus_lost"
    {
        return None;
    }
    let diagnostic = report
        .diagnostic
        .as_deref()
        .map(|diagnostic| format!(" ({diagnostic})"))
        .unwrap_or_default();
    Some(format!(
        "parakit: paste completed without restoring previous clipboard contents{diagnostic}"
    ))
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
    let outcome = with_injector(injector, |injector| {
        injector.stage_text_for_history(text, policy)
    })
    .context(copy_context)?;
    Ok(finish_staging(outcome, reason, log, notifier))
}

/// Report a clipboard fallback after its primary insertion diagnostic.
fn finish_staging(
    outcome: crate::daemon::inject::StageOutcome,
    reason: PasteBlockReason,
    log: &Logger,
    notifier: &Notifier,
) -> InsertReport {
    let worker_outcome = match &outcome {
        crate::daemon::inject::StageOutcome::CopiedOnly => {
            notifier.transcript_copied(reason.notice());
            InsertOutcome::CopiedOnly
        }
        crate::daemon::inject::StageOutcome::Blocked => {
            log.verbose(format!(
                "automatic paste skipped ({}); transcript staged for clipboard history",
                reason.log_tag()
            ));
            notifier.paste_blocked(reason.notice());
            InsertOutcome::Blocked
        }
        crate::daemon::inject::StageOutcome::ClipboardChanged(diagnostic) => {
            log_clipboard_changed(log, diagnostic.as_deref());
            notifier.paste_blocked(PasteBlockReason::ClipboardChanged.notice());
            InsertOutcome::Blocked
        }
    };
    InsertReport::from_paste(worker_outcome, outcome.into())
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
    keep_transcript_clipboard
        && mode != PasteMode::Direct
        && !error.is::<crate::daemon::inject::ClipboardRestoreError>()
}

fn clipboard_policy(keep_transcript_clipboard: bool) -> ClipboardPolicy {
    if keep_transcript_clipboard {
        ClipboardPolicy::KeepTranscript
    } else {
        ClipboardPolicy::RestorePrevious
    }
}

/// Check the original target and record its live verification label.
///
/// # Arguments
/// * `focus` - Original target and telemetry cell; missing targets record `"unavailable"`.
/// * `log` - Daemon logger used for diagnostics.
fn focus_allows_insertion(focus: FocusCheck<'_>, log: &Logger) -> bool {
    let Some(snapshot) = focus.snapshot else {
        focus.verification.set("unavailable");
        return unavailable_focus_allows_insertion(REQUIRE_VERIFIED_FOCUS, log);
    };

    focus_verification_allows_insertion(
        snapshot.verify_current(),
        focus.verification,
        REQUIRE_VERIFIED_FOCUS,
        log,
    )
}

fn unavailable_focus_allows_insertion(require_verified_focus: bool, log: &Logger) -> bool {
    if require_verified_focus {
        // macOS and Windows insertion must prove the current foreground target
        // still matches the hotkey target; unknown focus is not safe to paste.
        log.warn("recording focus was unavailable; automatic paste skipped");
        return false;
    }

    // Linux/X11 focus can be transiently unavailable. Preserve the existing
    // behavior there so a temporary X11 query failure does not drop speech.
    log.verbose("recording focus was unavailable; pasting without focus guard");
    true
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
        REQUIRE_VERIFIED_FOCUS,
        log,
    ))
}

fn focus_verification_allows_insertion(
    result: Result<FocusVerification>,
    verification: &Cell<&'static str>,
    require_verified_focus: bool,
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
        Err(err) if require_verified_focus => {
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

/// Why automatic insertion stopped, with log and notification descriptions.
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
    /// The paste chord was sent without acknowledgement and another process
    /// replaced the staged clipboard text before recovery.
    UnconfirmedClipboardChanged,
    /// Clipboard contents changed or could not be verified during insertion.
    ClipboardChanged,
    /// Native insertion rejected its captured target.
    #[cfg(target_os = "linux")]
    NativeRejected,
    /// Native insertion may have submitted text before losing its reply.
    #[cfg(target_os = "linux")]
    NativeUncertain,
    /// Terminal content cannot be sent through the input method automatically.
    #[cfg(target_os = "linux")]
    NativeTerminalControls,
}

impl PasteBlockReason {
    /// Return a lowercase insertion-stop reason for logs.
    fn log_tag(self) -> &'static str {
        match self {
            Self::EmptyAfterSanitization => "empty transcript after sanitization",
            Self::MultilineTerminal => "multiline terminal transcript",
            Self::BackendUnavailable => "paste backend unavailable",
            Self::FocusChangedBeforeInsertion => "focus changed before insertion",
            Self::FocusChangedBeforePaste => "focus changed immediately before paste",
            Self::UnsafeModifiers => "physical modifiers made the paste shortcut unsafe",
            Self::Unconfirmed => "paste not acknowledged",
            Self::UnconfirmedClipboardChanged => "paste not acknowledged and clipboard replaced",
            Self::ClipboardChanged => "clipboard changed or unavailable",
            #[cfg(target_os = "linux")]
            Self::NativeRejected => "native insertion rejected",
            #[cfg(target_os = "linux")]
            Self::NativeUncertain => "native delivery uncertain",
            #[cfg(target_os = "linux")]
            Self::NativeTerminalControls => "terminal content blocked native insertion",
        }
    }

    /// Return the insertion-stop explanation for desktop notifications.
    fn notice(self) -> &'static str {
        match self {
            Self::EmptyAfterSanitization => "Transcript was empty after cleanup.",
            Self::MultilineTerminal => "Multi-line transcript; terminal mode does not auto-paste.",
            Self::BackendUnavailable => "Paste backend was unavailable.",
            #[cfg(target_os = "linux")]
            Self::NativeRejected => "Native insertion was rejected. Recover the transcript from the clipboard or history.",
            #[cfg(target_os = "linux")]
            Self::NativeUncertain => "Native delivery is uncertain. Inspect the target before manually inserting the recovered transcript.",
            #[cfg(target_os = "linux")]
            Self::NativeTerminalControls => "Terminal content policy blocked automatic native insertion.",
            Self::FocusChangedBeforeInsertion => "Focus changed before insertion.",
            Self::FocusChangedBeforePaste => "Focus changed immediately before paste.",
            Self::UnsafeModifiers => {
                "Push-to-talk keys remained held; transcript copied for manual paste."
            }
            Self::ClipboardChanged => {
                "Clipboard changed or was unavailable; automatic insertion stopped."
            }
            Self::UnconfirmedClipboardChanged => {
                "Paste could not be confirmed and the clipboard changed. Inspect the target before retrying."
            }
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
    /// Native input method emitted text without application reception evidence.
    #[cfg(target_os = "linux")]
    Submitted,
    /// Native text may have been submitted before its reply was lost.
    #[cfg(target_os = "linux")]
    Uncertain,
    Pasted,
    /// Paste was sent without confirmation; telemetry records clipboard disposition.
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
            #[cfg(target_os = "linux")]
            Self::Submitted => "submitted",
            #[cfg(target_os = "linux")]
            Self::Uncertain => "uncertain",
            Self::Pasted => "pasted",
            Self::PastedUnverified => "pasted_unverified",
            Self::CopiedOnly => "copied_only",
            Self::Blocked => "blocked",
            Self::Skipped => "skipped",
        }
    }
}

/// Insertion outcome and observed acknowledgement/clipboard telemetry.
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

#[derive(Debug)]
struct ReportedInsertionError {
    message: String,
    report: InsertReport,
}

impl fmt::Display for ReportedInsertionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ReportedInsertionError {}

impl InsertReport {
    /// Retain a completed paste's acknowledgement and clipboard telemetry.
    fn from_paste(outcome: InsertOutcome, report: crate::daemon::inject::PasteReport) -> Self {
        Self {
            outcome,
            telemetry: report.telemetry,
            typed_chars: None,
            failure_reason: report.diagnostic,
        }
    }

    /// Build a report before acknowledgement or clipboard observations exist.
    fn placeholder(outcome: InsertOutcome, paste_event_posted: bool) -> Self {
        Self {
            outcome,
            telemetry: InsertionTelemetry::not_applicable(paste_event_posted, None),
            typed_chars: None,
            failure_reason: None,
        }
    }

    #[cfg(any(target_os = "linux", test))]
    fn direct_failure(typed_chars: usize, failure_reason: String) -> Self {
        Self {
            outcome: InsertOutcome::Blocked,
            telemetry: InsertionTelemetry::not_applicable(typed_chars > 0, None),
            typed_chars: Some(typed_chars),
            failure_reason: Some(failure_reason),
        }
    }

    /// Alarm for blocked, skipped, or uncertain delivery, and unconfirmed posted pastes.
    /// Copy-only recovery before any paste chord uses the quieter success cue.
    ///
    /// # Returns
    ///
    /// `true` when the daemon should play its error tone for this outcome.
    pub(super) fn needs_alert(&self) -> bool {
        let blocked = matches!(
            self.outcome,
            InsertOutcome::Blocked | InsertOutcome::Skipped
        );
        #[cfg(target_os = "linux")]
        let blocked = blocked || self.outcome == InsertOutcome::Uncertain;
        blocked
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
