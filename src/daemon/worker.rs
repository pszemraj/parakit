//! Transcription worker and paste safety boundary.

use anyhow::{Context, Result};
use crossbeam_channel::Receiver;
use parakit::data_log::DataLogger;
use parakit::inference::Engine;
use parakit::rules::Cleaner;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::audio::TARGET_RATE;
use super::inject::{FocusSnapshot, Injector, PasteMode};
use super::ipc::SharedState;
use super::logging::Logger;
use super::notifications::Notifier;
use super::sounds::Sounds;

/// Maximum number of worker events that may queue while ASR or paste is busy.
pub(crate) const WORKER_QUEUE_CAPACITY: usize = 2;

const PASTE_FAILURE_CIRCUIT_BREAKER: usize = 3;
const MAX_PASTE_CHARS: usize = 20_000;
const TERMINAL_MAX_PASTE_CHARS: usize = 2_000;
const SILENCE_PEAK_THRESHOLD: f32 = 0.001;
const SILENCE_RMS_THRESHOLD: f32 = 0.0005;

/// Events consumed by the transcription worker.
pub(crate) enum WorkerEvent {
    /// Recording began at this instant.
    RecordingStarted,
    /// Recording ended and the captured PCM moved out of the audio buffer.
    RecordingStopped {
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
    let mut consecutive_paste_failures = 0_usize;
    let mut copy_only_mode = false;
    while let Ok(ev) = rx.recv() {
        match ev {
            WorkerEvent::RecordingStarted => {
                state.set_phase("recording");
                sounds.start();
                log.line("parakit: recording...");
            }
            WorkerEvent::RecordingStopped {
                started_at,
                stopped_at,
                pcm,
                focus_at_start,
            } => {
                let stop_started = Instant::now();
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
                        state.set_last_transcript(transcript.cleaned.clone());
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
                            state.set_phase("idle");
                            sounds.success();
                            continue;
                        }
                        let insert_started = Instant::now();
                        let insert_result = match sanitize_for_paste(
                            &transcript.cleaned,
                            paste_mode,
                        ) {
                            PastePlan::Paste(text) if copy_only_mode => {
                                match copy_transcript_to_clipboard(&mut injector, &text)
                                    .context("clipboard-only mode copy failed")
                                {
                                    Ok(()) => {
                                        notifier.transcript_copied(
                                            "Paste is disabled for this session after repeated failures.",
                                        );
                                        Ok(InsertOutcome::CopiedOnly)
                                    }
                                    Err(err) => Err(err),
                                }
                            }
                            PastePlan::Paste(text) => paste_transcript(
                                &mut injector,
                                &text,
                                paste_mode,
                                focus_at_start.as_deref(),
                                &log,
                                &notifier,
                            ),
                            PastePlan::CopyOnly { text, reason } => {
                                log.warn(format!(
                                    "paste blocked by sanitizer ({reason}); transcript copied to clipboard"
                                ));
                                match copy_transcript_to_clipboard(&mut injector, &text)
                                    .context("sanitized transcript clipboard fallback failed")
                                {
                                    Ok(()) => {
                                        notifier.transcript_copied(reason);
                                        Ok(InsertOutcome::CopiedOnly)
                                    }
                                    Err(err) => Err(err),
                                }
                            }
                            PastePlan::Skip { reason } => {
                                log.warn(format!("paste skipped by sanitizer: {reason}"));
                                Ok(InsertOutcome::Skipped)
                            }
                        };
                        match insert_result {
                            Ok(outcome) => {
                                if outcome == InsertOutcome::Pasted {
                                    consecutive_paste_failures = 0;
                                }
                                let insert_elapsed = insert_started.elapsed();
                                log.verbose(format!(
                                    "parakit: timings infer={}ms clean={}ms insert={}ms total={}ms",
                                    transcript.infer_elapsed.as_secs_f32() * 1000.0,
                                    transcript.clean_elapsed.as_secs_f32() * 1000.0,
                                    insert_elapsed.as_secs_f32() * 1000.0,
                                    stop_started.elapsed().as_secs_f32() * 1000.0
                                ));
                                state.set_phase("idle");
                                if outcome == InsertOutcome::Blocked {
                                    sounds.error();
                                } else {
                                    sounds.success();
                                }
                            }
                            Err(e) => {
                                consecutive_paste_failures =
                                    consecutive_paste_failures.saturating_add(1);
                                if consecutive_paste_failures >= PASTE_FAILURE_CIRCUIT_BREAKER
                                    && !copy_only_mode
                                {
                                    copy_only_mode = true;
                                    notifier.paste_disabled_for_session();
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

fn paste_transcript(
    injector: &mut Option<Injector>,
    text: &str,
    mode: PasteMode,
    focus: Option<&FocusSnapshot>,
    log: &Logger,
    notifier: &Notifier,
) -> Result<InsertOutcome> {
    if !focus_allows_insertion(focus, log) {
        copy_transcript_to_clipboard(injector, text)
            .context("focus changed; transcript copied to clipboard fallback")?;
        notifier.transcript_copied("Focus changed before insertion.");
        return Ok(InsertOutcome::CopiedOnly);
    }

    match super::target::inspect_current_target() {
        super::target::TargetDecision::Allow => {}
        super::target::TargetDecision::CopyOnly(reason) => {
            log.warn(format!(
                "paste blocked by target safety ({reason}); transcript copied to clipboard"
            ));
            copy_transcript_to_clipboard(injector, text)
                .context("target safety clipboard fallback failed")?;
            notifier.transcript_copied(reason);
            return Ok(InsertOutcome::CopiedOnly);
        }
        super::target::TargetDecision::Block(reason) => {
            log.warn(format!(
                "paste blocked by target safety ({reason}); transcript was not copied"
            ));
            notifier.paste_blocked(reason);
            return Ok(InsertOutcome::Blocked);
        }
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
                "paste backend unavailable ({err:#}); transcript copied to clipboard"
            ));
            copy_transcript_to_clipboard(injector, text)
                .context("paste backend unavailable and clipboard fallback failed")?;
            notifier.transcript_copied("Paste backend was unavailable.");
            return Ok(InsertOutcome::CopiedOnly);
        }
        return Err(err.context("could not prepare direct insertion backend"));
    }

    let paste_result = injector
        .as_mut()
        .expect("insertion backend was just initialized")
        .paste_text_guarded(text, mode, || Ok(focus_allows_insertion(focus, log)));
    let paste_error = match paste_result {
        Ok(super::inject::PasteOutcome::Pasted) => return Ok(InsertOutcome::Pasted),
        Ok(super::inject::PasteOutcome::CopiedOnly) => {
            notifier.transcript_copied("Focus changed immediately before paste.");
            return Ok(InsertOutcome::CopiedOnly);
        }
        Err(err) => err,
    };

    if paste_failure_uses_clipboard_fallback(mode, &paste_error) {
        copy_transcript_to_clipboard(injector, text).map_err(|copy_error| {
            anyhow::anyhow!("{paste_error:#}; clipboard fallback also failed: {copy_error:#}")
        })?;
        return Err(
            paste_error.context("transcript copied to clipboard fallback after paste failure")
        );
    }

    Err(paste_error)
}

fn copy_transcript_to_clipboard(injector: &mut Option<Injector>, text: &str) -> Result<()> {
    match injector.as_mut() {
        Some(injector) => injector.copy_text(text),
        None => {
            let mut rebuilt = Injector::new()
                .context("could not initialize insertion backend for clipboard fallback")?;
            let result = rebuilt.copy_text(text);
            *injector = Some(rebuilt);
            result
        }
    }
}

fn paste_failure_uses_clipboard_fallback(mode: PasteMode, error: &anyhow::Error) -> bool {
    mode != PasteMode::Direct
        && !format!("{error:#}").contains(super::inject::CLIPBOARD_RESTORE_ERROR)
}

fn focus_allows_insertion(focus: Option<&FocusSnapshot>, log: &Logger) -> bool {
    let Some(focus) = focus else {
        log.warn("recording focus was unavailable; copied transcript to clipboard without pasting");
        return false;
    };

    match focus.matches_current() {
        Ok(true) => true,
        Ok(false) => {
            log.warn(
                "focus changed before insertion; copied transcript to clipboard without pasting",
            );
            false
        }
        Err(err) => {
            log.warn(format!(
                "could not verify recording focus ({err:#}); copied transcript to clipboard without pasting"
            ));
            false
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InsertOutcome {
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
/// A paste, copy-only, or skip decision.
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
    use super::*;

    #[test]
    fn paste_failures_use_clipboard_fallback_only_when_safe() {
        let paste_error = anyhow::anyhow!("could not send paste shortcut");
        assert!(paste_failure_uses_clipboard_fallback(
            PasteMode::Terminal,
            &paste_error
        ));

        let restore_error =
            anyhow::anyhow!("{}: lost", super::super::inject::CLIPBOARD_RESTORE_ERROR);
        assert!(!paste_failure_uses_clipboard_fallback(
            PasteMode::Terminal,
            &restore_error
        ));

        let direct_error = anyhow::anyhow!("could not type text at cursor");
        assert!(!paste_failure_uses_clipboard_fallback(
            PasteMode::Direct,
            &direct_error
        ));
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
