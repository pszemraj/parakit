//! Guarded clipboard-swap paste transaction: staging, final recheck, chord dispatch, restore.

use super::clipboard_guard::{ClipboardBeforeStaging, ClipboardRestore, StagedClipboard};
use super::clipboard_store::{ClipboardSnapshot, ClipboardStore};
use super::{
    ClipboardPolicy, FocusSnapshot, InsertionTelemetry, PasteDispatch, PasteMode, PasteOutcome,
    PasteReport, StageOutcome,
};
use crate::daemon::desktop::clipboard_restore::{
    sleep_if_nonzero, ClipboardRestoreGate, ClipboardRestorePlan, ClipboardWriteToken,
    PasteConfirmation, PasteConfirmationContext, PasteTargetValue,
};
use anyhow::{Context, Result};
use std::time::Duration;

/// Convert a completed clipboard-staging outcome into a [`PasteReport`].
///
/// Staging never sends a paste chord. [`StageOutcome::CopiedOnly`] means the
/// transcript was intentionally left on the clipboard (no restore);
/// [`StageOutcome::Blocked`] means the previous clipboard contents were
/// restored.
///
/// # Returns
///
/// A report that mirrors the staging result and records no posted paste event.
pub(super) fn report_from_stage_outcome(outcome: StageOutcome) -> PasteReport {
    match outcome {
        StageOutcome::CopiedOnly => PasteReport::new(PasteOutcome::CopiedOnly, false, Some(false)),
        StageOutcome::Blocked => PasteReport::new(PasteOutcome::Blocked, false, Some(true)),
        StageOutcome::ClipboardChanged(diagnostic) => PasteReport::clipboard_changed(diagnostic),
    }
}

/// Stage the transcript, recheck focus, send the paste chord, and restore the
/// previous clipboard per policy.
///
/// # Arguments
///
/// * `clipboard` - Clipboard backend to stage into and restore.
/// * `text` - Transcript text to paste.
/// * `mode` - Paste mode used to select safe acknowledgement evidence.
/// * `prepare_paste` - Waits for modifier readiness; `false` means it timed out.
/// * `paste` - Sends the paste chord.
/// * `settle_delay` - Delay between staging and the final focus recheck.
/// * `restore_plan` - Restore timing/acknowledgement policy.
/// * `clipboard_policy` - Whether the previous clipboard is restored afterwards.
/// * `focus` - Focus snapshot passed through to the acknowledgement strategy.
/// * `before_chord` - Safety recheck run before staging and again before the
///   chord; `Ok(false)` blocks insertion.
///
/// # Returns
///
/// A [`PasteReport`] describing whether a chord was sent and what happened to
/// the clipboard.
///
/// # Errors
///
/// Returns an error when a safety recheck or the paste dispatch fails, or when
/// the transcript cannot be written to the clipboard.
#[allow(
    clippy::too_many_arguments,
    reason = "each parameter is an independently meaningful piece of the guarded-paste transaction \
              (clipboard, transcript, modifier readiness, paste sender, timing, restore policy, \
              clipboard policy, focus context for acknowledgement, and the safety-recheck closure); \
              grouping them would just move the complexity into an ad hoc params struct with no real \
              callers besides this fn"
)]
pub(super) fn paste_with_clipboard_swap_guarded<C, R, P, G, H>(
    clipboard: &mut C,
    text: &str,
    mode: PasteMode,
    mut prepare_paste: R,
    mut paste: P,
    settle_delay: Duration,
    restore_plan: ClipboardRestorePlan<'_, H>,
    clipboard_policy: ClipboardPolicy,
    focus: Option<&FocusSnapshot>,
    mut before_chord: G,
) -> Result<PasteReport>
where
    C: ClipboardStore,
    R: FnMut() -> bool,
    P: FnMut() -> Result<PasteDispatch>,
    G: FnMut() -> Result<bool>,
    H: ClipboardRestoreGate + ?Sized,
{
    match before_chord() {
        Ok(true) => {}
        Ok(false) => {
            return stage_text_without_paste(clipboard, text, restore_plan, clipboard_policy)
                .map(report_from_stage_outcome);
        }
        Err(err) => return Err(err),
    }

    // The macOS and Linux backends may need to wait for the physical PTT
    // chord (or another modifier) to be released. Do that before staging and, most
    // importantly, before the final focus recheck below. A timed-out wait
    // leaves the transcript on the clipboard for manual recovery.
    if !prepare_paste() {
        stage_text_without_paste(
            clipboard,
            text,
            restore_plan,
            ClipboardPolicy::KeepTranscript,
        )?;
        return Ok(PasteReport::new(
            PasteOutcome::UnsafeModifiers,
            false,
            Some(false),
        ));
    }

    let (previous, mut clipboard_policy, mut retention_diagnostic) =
        previous_clipboard_for_policy(clipboard, clipboard_policy);
    let write_before = restore_plan.before_transcript_write();
    clipboard
        .set_text(text.to_owned())
        .context("could not copy transcript to clipboard")?;
    let mut previous = StagedClipboard::capture(clipboard, previous, text);
    let mut write_token = restore_plan.after_transcript_write(write_before);

    sleep_if_nonzero(settle_delay);

    // Capture the target value before the final focus recheck. macOS AX
    // reads are bounded but blocking; keeping them on this side of the guard
    // leaves no AX round-trip between the last focus decision and the chord.
    // The guard immediately below still proves the captured target remains
    // current before `paste()` posts any input.
    let baseline = restore_plan.capture_paste_baseline(focus);

    // Payload reads may block, so finish them before checking focus. A failed
    // observation calls for re-staging immediately before a safe chord, not
    // withholding the transcript or risking a paste of someone else's copy.
    let restage_reason = (!previous.is_current(clipboard)).then(|| {
        previous
            .observation_error()
            .unwrap_or_else(|| "clipboard changed after staging".to_owned())
    });

    let mut focus_check = before_chord();
    if matches!(focus_check, Ok(true)) {
        let restage_reason = restage_reason.or_else(|| {
            (!previous.stamp_is_current(clipboard)).then(|| {
                previous
                    .observation_error()
                    .unwrap_or_else(|| "clipboard changed during the final focus check".to_owned())
            })
        });
        if let Some(reason) = restage_reason {
            let write_before = restore_plan.before_transcript_write();
            clipboard
                .set_text(text.to_owned())
                .context("could not re-copy transcript to clipboard")?;
            previous = StagedClipboard::capture(clipboard, ClipboardSnapshot::Unsupported, text);
            write_token = restore_plan.after_transcript_write(write_before);
            clipboard_policy = ClipboardPolicy::KeepTranscript;
            retention_diagnostic = Some(reason);
            // The write and native stamp capture can block. Recheck focus
            // after them, without retrying the clipboard transaction.
            focus_check = before_chord();
        }
    }
    match focus_check {
        Ok(true) => {}
        Ok(false) => {
            return finish_blocked_clipboard(
                clipboard,
                previous,
                write_token,
                restore_plan,
                clipboard_policy,
            )
            .map(|report| report.with_retention_diagnostic(retention_diagnostic));
        }
        Err(err) => {
            let restore_result = restore_after_delay(
                clipboard,
                previous,
                write_token,
                restore_plan,
                clipboard_policy,
            );
            return match restore_result {
                Ok(ClipboardRestore::Changed(diagnostic)) => {
                    Ok(clipboard_changed_with_primary_error(err, diagnostic))
                }
                Ok(_) => Err(err),
                Err(restore_err) => Err(err.context(format!("{restore_err:#}"))),
            };
        }
    }
    // The restage can require another focus query. Preserve a copy observed
    // during that query instead of sending it to the target. Unreadable stamps
    // retain the existing fallback: stage once, then paste without restoring.
    // X11 stamps name the selection owner, not a write: a clipboard manager
    // re-owning the staged transcript is the handoff `is_current` accepts.
    if !cfg!(target_os = "linux")
        && !previous.stamp_is_current(clipboard)
        && previous.observation_error().is_none()
    {
        return finish_blocked_clipboard(
            clipboard,
            previous,
            write_token,
            restore_plan,
            clipboard_policy,
        )
        .map(|report| report.with_retention_diagnostic(retention_diagnostic));
    }
    let paste_result = paste();
    match paste_result {
        Ok(PasteDispatch::Posted) => Ok(finish_confirmed_paste(
            clipboard,
            previous,
            write_token,
            restore_plan,
            clipboard_policy,
            focus,
            text,
            mode,
            baseline.as_ref(),
        )
        .with_retention_diagnostic(retention_diagnostic)),
        Ok(PasteDispatch::SkippedUnsafeModifiers) => {
            // A live modifier made dispatch unsafe. Posting could change the
            // shortcut or release a held push-to-talk chord. No input was sent,
            // so leave the staged transcript on the clipboard for recovery.
            if !previous.is_current(clipboard) {
                retention_diagnostic = Some(previous.observation_error().unwrap_or_else(|| {
                    "clipboard changed while modifiers prevented paste".to_owned()
                }));
                clipboard
                    .set_text(text.to_owned())
                    .context("could not re-copy transcript to clipboard")?;
            }
            Ok(
                PasteReport::new(PasteOutcome::UnsafeModifiers, false, Some(false))
                    .with_retention_diagnostic(retention_diagnostic),
            )
        }
        Err(paste_err) => {
            let restore_result = restore_after_delay(
                clipboard,
                previous,
                write_token,
                restore_plan,
                clipboard_policy,
            );
            match restore_result {
                Ok(ClipboardRestore::Changed(diagnostic)) => {
                    Ok(clipboard_changed_with_primary_error(paste_err, diagnostic))
                }
                Ok(_) => Err(paste_err),
                Err(restore_err) => Err(paste_err.context(format!("{restore_err:#}"))),
            }
        }
    }
}

/// Resolve the outcome of a paste chord that was sent successfully: await
/// acknowledgement, then restore or retain the clipboard per policy and the
/// acknowledgement tier reached.
///
/// # Arguments
///
/// * `clipboard` - Clipboard backend to update.
/// * `previous` - Snapshot captured before staging transcript text.
/// * `write_token` - Clipboard write token for the staged transcript.
/// * `restore_plan` - Restore timing/acknowledgement policy.
/// * `clipboard_policy` - Policy deciding whether restoration should occur.
/// * `focus` - Focus snapshot passed through to the acknowledgement strategy.
/// * `text` - Transcript text that was just pasted.
/// * `mode` - Paste mode used to select safe acknowledgement evidence.
/// * `baseline` - Target's observable value read before the chord was sent.
///
/// Never fails: the paste chord was already sent by this point, so a
/// problem restoring the previous clipboard (see
/// [`clipboard_restored_after_paste`]) is reported through
/// `clipboard_restored: Some(false)` on the returned report rather than
/// turned into an error that would discard an already-landed paste.
///
/// [`PasteConfirmation::UnverifiedFocusLost`] and [`PasteConfirmation::NoEvidence`]
/// also report `clipboard_restored: Some(false)`, but deliberately: the
/// insertion target became unobservable (or never showed evidence) before a
/// restore could be trusted, so `previous` is dropped without being
/// restored when the transcript is still current. A superseding clipboard value
/// is preserved and reported with `clipboard_restored: None`. That is not a
/// restore failure and must not be logged as one — see the `daemon::worker`
/// call site that tells the two situations apart.
#[allow(
    clippy::too_many_arguments,
    reason = "mirrors the guarded-paste transaction's own parameter set; each is an \
              independently meaningful piece of resolving one paste"
)]
fn finish_confirmed_paste<C, H>(
    clipboard: &mut C,
    previous: StagedClipboard,
    write_token: ClipboardWriteToken,
    restore_plan: ClipboardRestorePlan<'_, H>,
    clipboard_policy: ClipboardPolicy,
    focus: Option<&FocusSnapshot>,
    text: &str,
    mode: PasteMode,
    baseline: Option<&PasteTargetValue>,
) -> PasteReport
where
    C: ClipboardStore,
    H: ClipboardRestoreGate + ?Sized,
{
    let confirmation = restore_plan.await_paste_confirmation(
        write_token,
        &PasteConfirmationContext {
            focus,
            transcript: text,
            mode,
            baseline,
        },
    );

    match confirmation {
        PasteConfirmation::Confirmed { elapsed, kind } => {
            let (clipboard_restored, diagnostic) =
                clipboard_restored_after_paste(clipboard, previous, clipboard_policy);
            PasteReport {
                outcome: PasteOutcome::Pasted,
                telemetry: InsertionTelemetry::acknowledged(kind, elapsed, clipboard_restored),
                diagnostic,
            }
        }
        PasteConfirmation::Unverified { elapsed, kind } => {
            let (clipboard_restored, diagnostic) =
                clipboard_restored_after_paste(clipboard, previous, clipboard_policy);
            PasteReport {
                outcome: PasteOutcome::PastedUnverified,
                telemetry: InsertionTelemetry::acknowledged(kind, elapsed, clipboard_restored),
                diagnostic,
            }
        }
        PasteConfirmation::UnverifiedFocusLost { elapsed, kind } => {
            // The chord was posted into a verified-focused target and very
            // likely landed, but the target became unobservable (app switch,
            // or focus state that can no longer be read) before any
            // evidence could appear. Unlike `Unverified`, where the same
            // target stays observable, that target can never be re-checked,
            // so `previous` is dropped here without being restored: the
            // transcript is the only remaining copy if the paste did not
            // land after all.
            let current = previous.is_current(clipboard);
            PasteReport {
                outcome: PasteOutcome::PastedUnverified,
                telemetry: InsertionTelemetry::acknowledged(
                    kind,
                    elapsed,
                    current.then_some(false),
                ),
                diagnostic: previous.observation_error(),
            }
        }
        PasteConfirmation::NoEvidence { elapsed, kind } => {
            // No evidence the target consumed the paste: `previous` is
            // dropped here without being restored, and the transcript
            // intentionally stays on the clipboard so it is not lost.
            // Uncertainty must never destroy the transcript.
            let current = previous.is_current(clipboard);
            PasteReport {
                outcome: if current {
                    PasteOutcome::CopiedOnly
                } else {
                    // The chord may have landed before another copy replaced
                    // the transcript. Reporting a pre-dispatch block invites
                    // a duplicate insertion from history.
                    PasteOutcome::PastedUnverified
                },
                telemetry: InsertionTelemetry::acknowledged(
                    kind,
                    elapsed,
                    current.then_some(false),
                ),
                diagnostic: previous.observation_error(),
            }
        }
    }
}

/// Restore the clipboard after a paste that already landed (or was accepted
/// as unverified), treating a failed restore as "not restored" rather than
/// turning an already-successful paste into an error.
///
/// # Returns
///
/// The restore telemetry plus a retained clipboard-observation diagnostic.
fn clipboard_restored_after_paste<C: ClipboardStore>(
    clipboard: &mut C,
    previous: StagedClipboard,
    clipboard_policy: ClipboardPolicy,
) -> (Option<bool>, Option<String>) {
    match previous.restore(clipboard, clipboard_policy) {
        Ok(ClipboardRestore::Restored) => (Some(true), None),
        Ok(ClipboardRestore::KeptTranscript) => (Some(false), None),
        Err(error) => (Some(false), Some(format!("{error:#}"))),
        Ok(ClipboardRestore::Changed(diagnostic)) => (None, diagnostic),
    }
}

fn clipboard_changed_with_primary_error(
    primary: anyhow::Error,
    clipboard_diagnostic: Option<String>,
) -> PasteReport {
    let diagnostic = match clipboard_diagnostic {
        Some(clipboard) => format!("{primary:#}; {clipboard}"),
        None => format!("{primary:#}"),
    };
    PasteReport::clipboard_changed(Some(diagnostic))
}

/// Save the clipboard for a later restore, or keep the transcript instead.
///
/// Delivering the dictation outranks restoring the previous clipboard, which
/// clipboard history managers retain anyway. A payload that cannot be read,
/// or that changes while it is read, switches only this transaction to
/// [`ClipboardPolicy::KeepTranscript`] rather than withholding the paste.
///
/// # Arguments
///
/// * `clipboard` - Clipboard backend to inspect.
/// * `clipboard_policy` - Requested retention policy.
///
/// # Returns
///
/// The snapshot to restore, the policy to apply, and the reason the
/// previous clipboard could not be saved, if any.
fn previous_clipboard_for_policy<C: ClipboardStore>(
    clipboard: &mut C,
    clipboard_policy: ClipboardPolicy,
) -> (ClipboardSnapshot, ClipboardPolicy, Option<String>) {
    if clipboard_policy == ClipboardPolicy::KeepTranscript {
        return (ClipboardSnapshot::Unsupported, clipboard_policy, None);
    }
    let unsaved = match ClipboardBeforeStaging::capture(clipboard) {
        Ok(previous) => match previous.is_current(clipboard) {
            Ok(true) => return (previous.snapshot, clipboard_policy, None),
            Ok(false) => "clipboard changed while it was being saved".to_owned(),
            Err(err) => format!("could not verify the saved clipboard: {err:#}"),
        },
        Err(err) => format!("could not save the previous clipboard: {err:#}"),
    };
    (
        ClipboardSnapshot::Unsupported,
        ClipboardPolicy::KeepTranscript,
        Some(unsaved),
    )
}

/// Stage the transcript on the clipboard without sending any paste input, then
/// apply the clipboard policy.
///
/// # Arguments
///
/// * `clipboard` - Clipboard backend to stage into and restore.
/// * `text` - Transcript text to stage.
/// * `restore_plan` - Restore timing/acknowledgement policy.
/// * `clipboard_policy` - Whether the previous clipboard is restored afterwards.
///
/// # Returns
///
/// [`StageOutcome::CopiedOnly`] when the transcript stays on the clipboard,
/// [`StageOutcome::Blocked`] when the previous clipboard was restored, or
/// [`StageOutcome::ClipboardChanged`] when another copy superseded it.
///
/// # Errors
///
/// Returns an error if the transcript cannot be written or the previous
/// clipboard cannot be restored.
pub(super) fn stage_text_without_paste<C, H>(
    clipboard: &mut C,
    text: &str,
    restore_plan: ClipboardRestorePlan<'_, H>,
    clipboard_policy: ClipboardPolicy,
) -> Result<StageOutcome>
where
    C: ClipboardStore,
    H: ClipboardRestoreGate + ?Sized,
{
    let (previous, clipboard_policy, _) =
        previous_clipboard_for_policy(clipboard, clipboard_policy);
    if clipboard_policy == ClipboardPolicy::KeepTranscript {
        clipboard
            .set_text(text.to_owned())
            .context("could not copy transcript to clipboard")?;
        return Ok(StageOutcome::CopiedOnly);
    }

    let write_before = restore_plan.before_transcript_write();
    clipboard
        .set_text(text.to_owned())
        .context("could not copy transcript to clipboard")?;
    let previous = StagedClipboard::capture(clipboard, previous, text);
    let write_token = restore_plan.after_transcript_write(write_before);
    let restored = restore_after_delay(
        clipboard,
        previous,
        write_token,
        restore_plan,
        clipboard_policy,
    )?;
    Ok(match restored {
        ClipboardRestore::Changed(diagnostic) => StageOutcome::ClipboardChanged(diagnostic),
        _ => StageOutcome::Blocked,
    })
}

fn finish_blocked_clipboard<C, H>(
    clipboard: &mut C,
    previous: StagedClipboard,
    write_token: ClipboardWriteToken,
    restore_plan: ClipboardRestorePlan<'_, H>,
    clipboard_policy: ClipboardPolicy,
) -> Result<PasteReport>
where
    C: ClipboardStore,
    H: ClipboardRestoreGate + ?Sized,
{
    let restored = restore_after_delay(
        clipboard,
        previous,
        write_token,
        restore_plan,
        clipboard_policy,
    )?;
    Ok(match restored {
        ClipboardRestore::Restored => PasteReport::new(PasteOutcome::Blocked, false, Some(true)),
        ClipboardRestore::KeptTranscript => {
            PasteReport::new(PasteOutcome::CopiedOnly, false, Some(false))
        }
        ClipboardRestore::Changed(diagnostic) => PasteReport::clipboard_changed(diagnostic),
    })
}

/// Wait for the fallback/history-based restore signal, then restore or
/// clear the clipboard per policy.
///
/// Used by every guarded-cancellation and error path that never reaches a
/// paste chord (and therefore never has paste-acknowledgement evidence to
/// consult): the guard-blocked-after-staging path, the post-settle guard
/// recheck failure path, the paste-error path, and plain staging. The
/// post-paste-chord success path uses
/// [`ClipboardRestorePlan::await_paste_confirmation`] instead (see
/// [`finish_confirmed_paste`]), since by then a paste chord was actually
/// sent and a real acknowledgement signal may be available.
///
/// # Errors
///
/// Returns an error if the previous clipboard payload cannot be restored.
fn restore_after_delay<C, H>(
    clipboard: &mut C,
    previous: StagedClipboard,
    write_token: ClipboardWriteToken,
    restore_plan: ClipboardRestorePlan<'_, H>,
    clipboard_policy: ClipboardPolicy,
) -> Result<ClipboardRestore>
where
    C: ClipboardStore,
    H: ClipboardRestoreGate + ?Sized,
{
    if clipboard_policy == ClipboardPolicy::RestorePrevious {
        restore_plan.wait_before_restore(write_token);
    }
    previous.restore(clipboard, clipboard_policy)
}
