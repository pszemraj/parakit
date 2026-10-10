//! Transcription logging for collecting raw/cleaned cleanup pairs.

use crate::rules::RuleHit;
use anyhow::{Context, Result};
use chrono::{Local, NaiveDate, SecondsFormat, Utc};
use fs2::FileExt;
use parking_lot::Mutex;
use serde::Serialize;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Identifier returned by [`DataLogger::log`] that correlates a
/// transcription record with its later insertion outcome recorded through
/// [`DataLogger::log_insertion`].
#[derive(Clone, Debug)]
pub struct RecordId {
    session_id: Arc<str>,
    sequence: u64,
    local_date: NaiveDate,
}

/// Transcript-cleaning telemetry recorded with the transcription record.
///
/// Every field describes how [`DataLogger::log`]'s `cleaned` argument was
/// derived from its `raw` argument, so downstream analysis can join a
/// transcription record with the cleaning behavior that produced it.
#[derive(Serialize)]
pub struct CleaningLogFields<'a> {
    /// Number of enabled passes after profile and disable filtering.
    pub rules_active: usize,
    /// Cleaner schema/behavior version (`parakit::rules::CLEANER_VERSION`).
    pub cleaner_version: u32,
    /// Selected profile: "safe", "aggressive", or "disabled" when cleaning is off.
    #[serde(rename = "cleaning_profile")]
    pub profile: &'static str,
    /// Stable identifier of the ordered enabled pass set; None when cleaning is off.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ruleset_id: Option<&'a str>,
    /// Whether the messaging-style terminal-period pass was enabled.
    pub drops_trailing_period: bool,
    /// Minimum isolated number converted to digits; `None` when cleaning is disabled.
    pub number_threshold: Option<f64>,
    /// Transformations that actually changed the transcript, in application order.
    pub rules_fired: &'a [RuleHit],
    /// Set when a cleaning pass failed at runtime and the transcript was passed through unchanged.
    #[serde(rename = "cleaning_failure", skip_serializing_if = "Option::is_none")]
    pub failure: Option<&'a str>,
}

#[derive(Serialize)]
struct LogRecord<'a> {
    ts: String,
    session_id: &'a str,
    record_id: u64,
    parakit_version: &'static str,
    audio_secs: f32,
    infer_ms: u128,
    raw: &'a str,
    cleaned: &'a str,
    #[serde(flatten)]
    cleaning: CleaningLogFields<'a>,
}

/// Insertion-outcome telemetry correlated with a previously logged
/// transcription record.
///
/// Every field describes what happened after the record identified by a
/// [`RecordId`] was written, so downstream analysis can join a transcription
/// record with how (or whether) it reached the focused application.
#[derive(Serialize)]
pub struct InsertionLogFields<'a> {
    /// Coarse insertion result, e.g. `"pasted"`, `"pasted_unverified"`,
    /// `"copied_only"`, `"blocked"`, `"skipped"`, or `"error"`.
    pub outcome: &'static str,
    /// Bundle identifier of the insertion target, when known.
    pub target_bundle_id: Option<&'a str>,
    /// How focus was verified before insertion: `"matched"`, `"changed"`,
    /// `"unavailable"`, `"ax_unsupported"`, or `"not_applicable"`.
    pub focus_verification: &'static str,
    /// Character count of the transcript offered for insertion.
    pub transcript_chars: usize,
    /// Whether a synthetic paste chord or type event was actually sent.
    pub paste_event_posted: bool,
    /// Reserved for a future clipboard read-back confirmation signal.
    pub pasteboard_requested: Option<bool>,
    /// How insertion success was acknowledged: `"ax_confirmed"`,
    /// `"unverified_timeout"`, `"unverified_no_baseline"`,
    /// `"unverified_focus_lost"`, `"no_evidence"`, or `"not_applicable"`.
    pub acknowledgement_kind: &'static str,
    /// Milliseconds spent waiting for acknowledgement, when applicable.
    pub acknowledgement_ms: Option<u128>,
    /// Whether the previous clipboard contents were restored after insertion.
    pub clipboard_restored: Option<bool>,
    /// Human-readable error or degraded-outcome diagnostic.
    pub failure_reason: Option<&'a str>,
}

#[derive(Serialize)]
struct InsertionLogRecord<'a> {
    kind: &'static str,
    ts: String,
    session_id: &'a str,
    ref_id: u64,
    #[serde(flatten)]
    fields: InsertionLogFields<'a>,
}

struct LogState {
    date: NaiveDate,
    file: File,
}

struct LogTimestamp {
    record_id: RecordId,
    utc_rfc3339: String,
}

impl LogTimestamp {
    fn now(session_id: Arc<str>, sequence: u64) -> Self {
        let now = Local::now();
        Self {
            record_id: RecordId {
                session_id,
                sequence,
                local_date: now.date_naive(),
            },
            utc_rfc3339: now
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true),
        }
    }
}

/// Synchronous JSONL transcription logger with lazy daily file rotation.
pub struct DataLogger {
    dir: PathBuf,
    session_id: Arc<str>,
    state: Mutex<Option<LogState>>,
    next_id: AtomicU64,
}

static NEXT_LOGGER_SESSION: AtomicU64 = AtomicU64::new(0);

impl DataLogger {
    /// Build a JSONL logger for `dir`.
    ///
    /// Files are opened lazily on the first write.
    ///
    /// # Arguments
    ///
    /// * `dir` - Directory that will receive daily log files.
    ///
    /// # Returns
    ///
    /// A logger ready to write records.
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            session_id: new_session_id(),
            state: Mutex::new(None),
            next_id: AtomicU64::new(0),
        }
    }

    /// Write one transcription record.
    ///
    /// Logging failures are printed to stderr and never propagated to the
    /// caller, because logging must not crash dictation. The JSONL record is
    /// flushed before this method returns.
    ///
    /// # Arguments
    ///
    /// * `audio_secs` - Duration of the recorded utterance in seconds.
    /// * `infer` - Time spent running model inference.
    /// * `raw` - Raw transcript returned by the model.
    /// * `cleaned` - Transcript after cleanup rules were applied.
    /// * `cleaning` - Cleaning telemetry describing how `cleaned` was derived
    ///   from `raw`.
    ///
    /// # Returns
    ///
    /// `Some` identifier that correlates this record with its insertion
    /// outcome when the record was written, or `None` when logging failed.
    pub fn log(
        &self,
        audio_secs: f32,
        infer: Duration,
        raw: &str,
        cleaned: &str,
        cleaning: CleaningLogFields<'_>,
    ) -> Option<RecordId> {
        let timestamp = LogTimestamp::now(
            Arc::clone(&self.session_id),
            self.next_id.fetch_add(1, Ordering::Relaxed),
        );
        let id = timestamp.record_id.clone();
        match self.try_log(timestamp, audio_secs, infer, raw, cleaned, cleaning) {
            Ok(()) => Some(id),
            Err(e) => {
                eprintln!("parakit: transcription log write failed: {e:#}");
                None
            }
        }
    }

    /// Write the insertion outcome for a record previously returned by
    /// [`DataLogger::log`].
    ///
    /// This appends a second, independently parseable JSONL line carrying
    /// `"kind":"insertion"`, the original `"session_id"`, and `"ref_id"`
    /// set to the original record's sequence.
    ///
    /// Logging failures are printed to stderr and never propagated, for the
    /// same reason as [`DataLogger::log`].
    ///
    /// # Arguments
    ///
    /// * `id` - Identifier returned by the original [`DataLogger::log`] call.
    /// * `fields` - Insertion telemetry to record.
    pub fn log_insertion(&self, id: &RecordId, fields: InsertionLogFields<'_>) {
        if let Err(e) = self.try_log_insertion(id, fields) {
            eprintln!("parakit: insertion log write failed: {e:#}");
        }
    }

    fn try_log(
        &self,
        timestamp: LogTimestamp,
        audio_secs: f32,
        infer: Duration,
        raw: &str,
        cleaned: &str,
        cleaning: CleaningLogFields<'_>,
    ) -> Result<()> {
        let record = LogRecord {
            ts: timestamp.utc_rfc3339,
            session_id: &timestamp.record_id.session_id,
            record_id: timestamp.record_id.sequence,
            parakit_version: crate::build_info::PACKAGE_VERSION,
            audio_secs,
            infer_ms: infer.as_millis(),
            raw,
            cleaned,
            cleaning,
        };

        self.with_state(timestamp.record_id.local_date, |state| {
            write_jsonl_record(state, &record, "failed to serialize jsonl log record")
        })
    }

    fn try_log_insertion(&self, id: &RecordId, fields: InsertionLogFields<'_>) -> Result<()> {
        let ts = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let record = InsertionLogRecord {
            kind: "insertion",
            ts,
            session_id: &id.session_id,
            ref_id: id.sequence,
            fields,
        };
        self.with_state(id.local_date, |state| {
            write_jsonl_record(state, &record, "failed to serialize jsonl insertion record")
        })
    }

    /// Run `f` against the requested daily log file, rotating first when the
    /// previous write targeted a different date.
    fn with_state<F>(&self, local_date: NaiveDate, f: F) -> Result<()>
    where
        F: FnOnce(&mut LogState) -> Result<()>,
    {
        let mut state = self.state.lock();
        if state.as_ref().map(|s| s.date) != Some(local_date) {
            *state = Some(LogState {
                date: local_date,
                file: self.open_for_date(local_date)?,
            });
        }
        let result = f(state
            .as_mut()
            .expect("log state is initialized or open_for_date returned an error"));
        if result.is_err() {
            *state = None;
        }
        result
    }

    fn open_for_date(&self, date: NaiveDate) -> Result<File> {
        create_dir_all(&self.dir)
            .with_context(|| format!("failed to create log dir {}", self.dir.display()))?;
        let path = self.dir.join(file_name(date));
        let mut options = OpenOptions::new();
        options.create(true);
        // Preserve OS append positioning across independent Unix loggers.
        #[cfg(not(windows))]
        options.append(true);
        // Windows append-only handles cannot truncate a failed partial write.
        #[cfg(windows)]
        options.write(true).truncate(false);
        let file = options
            .open(&path)
            .with_context(|| format!("failed to open log file {}", path.display()))?;
        Ok(file)
    }
}

fn new_session_id() -> Arc<str> {
    let started_at = Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true);
    let ordinal = NEXT_LOGGER_SESSION.fetch_add(1, Ordering::Relaxed);
    Arc::from(format!("{started_at}-p{}-l{ordinal}", std::process::id()))
}

fn write_jsonl_record<T: Serialize>(
    state: &mut LogState,
    record: &T,
    serialization_context: &'static str,
) -> Result<()> {
    let mut line = serde_json::to_vec(record).context(serialization_context)?;
    line.push(b'\n');
    append_with_rollback(&mut state.file, |file| {
        file.write_all(&line)?;
        file.flush()
    })
    .context("failed to append complete jsonl record")
}

/// Serialize append/rollback across logger handles sharing the daily file.
fn append_with_rollback<F>(file: &mut File, write: F) -> std::io::Result<()>
where
    F: FnOnce(&mut File) -> std::io::Result<()>,
{
    FileExt::lock_exclusive(file)?;
    let result = (|| {
        let original_len = file.seek(SeekFrom::End(0))?;
        if let Err(error) = write(file) {
            file.set_len(original_len)?;
            return Err(error);
        }
        Ok(())
    })();
    let unlocked = FileExt::unlock(file);
    result.and(unlocked)
}

fn file_name(date: NaiveDate) -> String {
    format!("parakit-{}.jsonl", date.format("%Y-%m-%d"))
}

#[cfg(test)]
#[path = "data_log_tests.rs"]
mod tests;
