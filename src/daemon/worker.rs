//! Transcription worker and paste safety boundary.

use anyhow::{Context, Result};
use crossbeam_channel::Receiver;
use parakit::data_log::DataLogger;
use parakit::inference::Engine;
use parakit::rules::Cleaner;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::audio::TARGET_RATE;
use super::inject::{ClipboardPolicy, FocusSnapshot, Injector, PasteMode};
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

    let rules_active = cleaner.as_deref().map_or(0, Cleaner::active_rule_count);
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
                        if let Some(data_log) = &data_log {
                            data_log.log(
                                secs,
                                transcript.infer_elapsed,
                                &transcript.raw,
                                &transcript.cleaned,
                                rules_active,
                            );
                        }
                        log.transcript(
                            &transcript.raw,
                            &transcript.cleaned,
                            transcript.infer_elapsed,
                        );
                        if !insert_transcripts {
                            log.verbose("parakit: insertion skipped for PTT audio simulation");
                            state.set_last_transcript(transcript.cleaned.clone());
                            state.set_phase("idle");
                            sounds.success();
                            continue;
                        }
                        let insert_started = Instant::now();
                        let cleaned = transcript.cleaned.clone();
                        paste_circuit.maybe_reenable(Instant::now(), log.as_ref());
                        let insert_result = state.with_insertion_lock(|| {
                            let result = insert_text(
                                &mut injector,
                                &cleaned,
                                paste_mode,
                                keep_transcript_clipboard,
                                focus_at_start.as_deref(),
                                (log.as_ref(), &notifier),
                                paste_circuit.copy_only_mode,
                            );
                            if insertion_result_remembers_transcript(&result) {
                                state.set_last_transcript(cleaned);
                            }
                            result
                        });
                        match insert_result {
                            Ok(outcome) => {
                                if outcome == InsertOutcome::Pasted {
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
                                if outcome == InsertOutcome::Blocked {
                                    sounds.error();
                                } else {
                                    sounds.success();
                                }
                            }
                            Err(e) => {
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

fn insertion_result_remembers_transcript(result: &Result<InsertOutcome>) -> bool {
    !matches!(result, Ok(InsertOutcome::Skipped))
}

/// Sanitize text and run the shared paste/copy insertion transaction.
///
/// # Arguments
///
/// * `injector` - Reused insertion backend, created lazily when needed.
/// * `raw_text` - Candidate transcript or IPC text.
/// * `mode` - Paste mode used for sanitizer policy and chord selection.
/// * `keep_transcript_clipboard` - Leave text on clipboard instead of restoring previous contents.
/// * `focus` - Focus snapshot captured before insertion became eligible.
/// * `ui` - Daemon logger and desktop notification wrapper.
/// * `copy_only_mode` - Circuit-breaker flag that disables synthetic paste.
///
/// # Returns
///
/// Worker insertion outcome.
///
/// # Errors
///
/// Returns an error if clipboard staging, injector initialization, or paste fails.
pub(crate) fn insert_text(
    injector: &mut Option<Injector>,
    raw_text: &str,
    mode: PasteMode,
    keep_transcript_clipboard: bool,
    focus: Option<&FocusSnapshot>,
    ui: (&Logger, &Notifier),
    copy_only_mode: bool,
) -> Result<InsertOutcome> {
    let (log, notifier) = ui;
    match sanitize_for_paste(raw_text, mode) {
        PastePlan::Paste(text) if copy_only_mode => {
            if mode == PasteMode::Direct {
                log.warn(
                    "direct insertion disabled after repeated failures; transcript was not copied",
                );
                notifier.paste_temporarily_disabled();
                Ok(InsertOutcome::Blocked)
            } else {
                copy_or_block_transcript(
                    injector,
                    &text,
                    keep_transcript_clipboard,
                    "paste-disabled clipboard copy failed",
                    "Paste is temporarily disabled after repeated failures.",
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
                    "direct insertion blocked by sanitizer ({reason}); transcript was not copied"
                ));
                notifier.paste_blocked(reason);
                Ok(InsertOutcome::Blocked)
            } else {
                log.warn(format!("paste blocked by sanitizer ({reason})"));
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
            log.warn(format!("paste skipped by sanitizer: {reason}"));
            Ok(InsertOutcome::Skipped)
        }
    }
}

fn paste_transcript(
    injector: &mut Option<Injector>,
    text: &str,
    mode: PasteMode,
    keep_transcript_clipboard: bool,
    focus: Option<&FocusSnapshot>,
    log: &Logger,
    notifier: &Notifier,
) -> Result<InsertOutcome> {
    if !focus_allows_insertion(focus, log) {
        if mode == PasteMode::Direct {
            notifier.paste_blocked("Focus changed before insertion.");
            return Ok(InsertOutcome::Blocked);
        }
        return copy_or_block_transcript(
            injector,
            text,
            keep_transcript_clipboard,
            "focus changed clipboard fallback failed",
            "Focus changed before insertion.",
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
                "Paste backend was unavailable.",
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
            || Ok(focus_allows_insertion(focus, log)),
        );
    let paste_error = match paste_result {
        Ok(super::inject::PasteOutcome::Pasted) => return Ok(InsertOutcome::Pasted),
        Ok(super::inject::PasteOutcome::CopiedOnly) => {
            notifier.transcript_copied("Focus changed immediately before paste.");
            return Ok(InsertOutcome::CopiedOnly);
        }
        Ok(super::inject::PasteOutcome::Blocked) => {
            notifier.paste_blocked("Focus changed immediately before paste.");
            return Ok(InsertOutcome::Blocked);
        }
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
    reason: &'static str,
    log: &Logger,
    notifier: &Notifier,
) -> Result<InsertOutcome> {
    match stage_transcript_for_history(injector, text, keep_transcript_clipboard)
        .context(copy_context)?
    {
        super::inject::StageOutcome::CopiedOnly => {
            notifier.transcript_copied(reason);
            Ok(InsertOutcome::CopiedOnly)
        }
        super::inject::StageOutcome::Blocked => {
            log.warn(format!(
                "automatic paste skipped ({reason}); transcript staged for clipboard history"
            ));
            notifier.paste_blocked(reason);
            Ok(InsertOutcome::Blocked)
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

fn focus_allows_insertion(focus: Option<&FocusSnapshot>, log: &Logger) -> bool {
    let Some(focus) = focus else {
        if cfg!(target_os = "windows") {
            // Windows insertion must prove the current foreground target still
            // matches the hotkey target; unknown focus is not safe to paste.
            log.warn("recording focus was unavailable; automatic paste skipped");
            return false;
        }

        // Linux/X11 focus can be transiently unavailable. Preserve the existing
        // behavior there so a temporary X11 query failure does not drop speech.
        log.verbose("recording focus was unavailable; pasting without focus guard");
        return true;
    };

    focus_verification_allows_insertion(focus.matches_current(), log)
}

fn focus_verification_allows_insertion(result: Result<bool>, log: &Logger) -> bool {
    match result {
        Ok(true) => true,
        Ok(false) => {
            log.warn("focus changed before insertion; automatic paste skipped");
            false
        }
        Err(err) if cfg!(target_os = "windows") => {
            log.warn(format!(
                "could not verify recording focus ({err:#}); automatic paste skipped"
            ));
            false
        }
        Err(err) => {
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
        /// Short user-facing reason.
        reason: &'static str,
    },
    /// Text should not be copied or pasted.
    Skip {
        /// Short user-facing reason.
        reason: &'static str,
    },
}

/// Result of a worker-level insertion attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InsertOutcome {
    Pasted,
    CopiedOnly,
    Blocked,
    Skipped,
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
            reason: "empty transcript after sanitization",
        };
    }

    if mode == PasteMode::Terminal {
        while text.ends_with('\n') {
            text.pop();
        }
        if text.trim().is_empty() {
            return PastePlan::Skip {
                reason: "empty terminal transcript after sanitization",
            };
        }
        if text.contains('\n') {
            return PastePlan::CopyOnly {
                text,
                reason: "multiline terminal transcript",
            };
        }
        if text.chars().count() > TERMINAL_MAX_PASTE_CHARS {
            return PastePlan::CopyOnly {
                text,
                reason: "terminal transcript too long",
            };
        }
    }

    if text.chars().count() > MAX_PASTE_CHARS {
        return PastePlan::CopyOnly {
            text,
            reason: "transcript too long",
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
    let cleaned = match cleaner {
        Some(c) => c.clean(&raw),
        None => raw.clone(),
    };
    Ok(Some(TranscriptResult {
        raw,
        cleaned,
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

        assert!(focus_verification_allows_insertion(Ok(true), &log));
        assert!(!focus_verification_allows_insertion(Ok(false), &log));

        #[cfg(target_os = "windows")]
        {
            assert!(!focus_allows_insertion(None, &log));
            assert!(!focus_verification_allows_insertion(
                Err(anyhow::anyhow!("focus unavailable")),
                &log
            ));
        }

        #[cfg(not(target_os = "windows"))]
        {
            assert!(focus_allows_insertion(None, &log));
            assert!(focus_verification_allows_insertion(
                Err(anyhow::anyhow!("temporary X11 failure")),
                &log
            ));
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
        assert!(insertion_result_remembers_transcript(&Ok(
            InsertOutcome::Pasted
        )));
        assert!(insertion_result_remembers_transcript(&Ok(
            InsertOutcome::CopiedOnly
        )));
        assert!(insertion_result_remembers_transcript(&Ok(
            InsertOutcome::Blocked
        )));
        assert!(insertion_result_remembers_transcript(&Err(
            anyhow::anyhow!("paste failed")
        )));
        assert!(!insertion_result_remembers_transcript(&Ok(
            InsertOutcome::Skipped
        )));
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
                    reason: "multiline terminal transcript",
                },
            ),
            (
                "empty standard skip",
                "\0\x07\n".to_string(),
                PasteMode::Standard,
                PastePlan::Skip {
                    reason: "empty transcript after sanitization",
                },
            ),
            (
                "long terminal copy-only",
                raw.clone(),
                PasteMode::Terminal,
                PastePlan::CopyOnly {
                    text: raw,
                    reason: "terminal transcript too long",
                },
            ),
        ];

        for (name, raw, mode, expected) in cases {
            assert_eq!(sanitize_for_paste(&raw, mode), expected, "{name}");
        }
    }
}
