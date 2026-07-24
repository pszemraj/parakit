//! Transcription logging for collecting raw/cleaned cleanup pairs.

use anyhow::{Context, Result};
use chrono::{Local, NaiveDate, SecondsFormat, Utc};
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashMap;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// On-disk format used for transcription logs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogFormat {
    /// Newline-delimited JSON records.
    Jsonl,
    /// Tab-separated records.
    Tsv,
}

impl std::str::FromStr for LogFormat {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "jsonl" | "json" => Ok(Self::Jsonl),
            "tsv" => Ok(Self::Tsv),
            other => Err(anyhow::anyhow!(
                "unknown log format '{other}'. Expected 'jsonl' or 'tsv'"
            )),
        }
    }
}

/// Identifier returned by [`DataLogger::log`] that correlates a
/// transcription record with its later insertion outcome recorded through
/// [`DataLogger::log_insertion`].
#[derive(Clone, Copy, Debug)]
pub struct RecordId(u64);

#[derive(Debug, Serialize)]
struct LogRecord<'a> {
    ts: String,
    audio_secs: f32,
    infer_ms: u128,
    raw: &'a str,
    cleaned: &'a str,
    rules_active: usize,
}

/// Insertion-outcome telemetry correlated with a previously logged
/// transcription record.
///
/// Every field describes what happened after the record identified by a
/// [`RecordId`] was written, so downstream analysis can join a transcription
/// record with how (or whether) it reached the focused application.
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
    /// `"unverified_timeout"`, `"no_evidence"`, or `"not_applicable"`.
    pub acknowledgement_kind: &'static str,
    /// Milliseconds spent waiting for acknowledgement, when applicable.
    pub acknowledgement_ms: Option<u128>,
    /// Whether the previous clipboard contents were restored after insertion.
    pub clipboard_restored: Option<bool>,
    /// Human-readable failure reason when insertion errored.
    pub failure_reason: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct InsertionLogRecord<'a> {
    kind: &'static str,
    ts: String,
    ref_id: u64,
    outcome: &'a str,
    target_bundle_id: Option<&'a str>,
    focus_verification: &'a str,
    transcript_chars: usize,
    paste_event_posted: bool,
    pasteboard_requested: Option<bool>,
    acknowledgement_kind: &'a str,
    acknowledgement_ms: Option<u128>,
    clipboard_restored: Option<bool>,
    failure_reason: Option<&'a str>,
}

/// Number of trailing TSV columns appended by an insertion record.
const INSERTION_TSV_COLUMNS: usize = 10;

/// Maximum time a TSV transcription row waits for its insertion outcome
/// before it is flushed with empty insertion columns.
const PENDING_TSV_MAX_AGE: Duration = Duration::from_secs(30);

struct LogState {
    date: NaiveDate,
    file: BufWriter<File>,
}

/// A TSV transcription row buffered until its insertion outcome is known.
struct PendingTsvRow {
    queued_at: Instant,
    prefix: String,
}

/// Synchronous transcription logger with lazy daily file rotation.
pub struct DataLogger {
    dir: PathBuf,
    format: LogFormat,
    state: Mutex<Option<LogState>>,
    next_id: AtomicU64,
    /// TSV rows written by [`DataLogger::log`] but not yet completed by a
    /// matching [`DataLogger::log_insertion`] call. Unused for JSONL, which
    /// correlates the two records by `ref_id` instead of by buffering.
    pending_tsv: Mutex<HashMap<u64, PendingTsvRow>>,
}

impl DataLogger {
    /// Build a logger for `dir` using the requested format.
    ///
    /// Files are opened lazily on the first write.
    ///
    /// # Arguments
    ///
    /// * `dir` - Directory that will receive daily log files.
    /// * `format` - File format to use for new log records.
    ///
    /// # Returns
    ///
    /// A logger ready to write records.
    pub fn new(dir: PathBuf, format: LogFormat) -> Self {
        Self {
            dir,
            format,
            state: Mutex::new(None),
            next_id: AtomicU64::new(0),
            pending_tsv: Mutex::new(HashMap::new()),
        }
    }

    /// Write one transcription record.
    ///
    /// Logging failures are printed to stderr and never propagated to the
    /// caller, because logging must not crash or block dictation. For the
    /// TSV format, the row is buffered rather than written immediately; see
    /// [`DataLogger::log_insertion`].
    ///
    /// # Arguments
    ///
    /// * `audio_secs` - Duration of the recorded utterance in seconds.
    /// * `infer` - Time spent running model inference.
    /// * `raw` - Raw transcript returned by the model.
    /// * `cleaned` - Transcript after cleanup rules were applied.
    /// * `rules_active` - Number of cleanup rules active for this transcript.
    ///
    /// # Returns
    ///
    /// An identifier that correlates this record with its insertion outcome.
    pub fn log(
        &self,
        audio_secs: f32,
        infer: Duration,
        raw: &str,
        cleaned: &str,
        rules_active: usize,
    ) -> RecordId {
        let id = RecordId(self.next_id.fetch_add(1, Ordering::Relaxed));
        if let Err(e) = self.try_log(id, audio_secs, infer, raw, cleaned, rules_active) {
            eprintln!("parakit: transcription log write failed: {e:#}");
        }
        id
    }

    /// Write the insertion outcome for a record previously returned by
    /// [`DataLogger::log`].
    ///
    /// For JSONL, this appends a second, independently parseable line
    /// carrying `"kind":"insertion"` and `"ref_id"` set to the original
    /// record's identifier. For TSV, this completes the buffered row from
    /// `log` by appending fixed-order trailing columns and writing the row;
    /// calling this for an id whose row already aged out (see
    /// [`PENDING_TSV_MAX_AGE`]) is a no-op, since that row was already
    /// flushed with empty insertion columns.
    ///
    /// Logging failures are printed to stderr and never propagated, for the
    /// same reason as [`DataLogger::log`].
    ///
    /// # Arguments
    ///
    /// * `id` - Identifier returned by the original [`DataLogger::log`] call.
    /// * `fields` - Insertion telemetry to record.
    pub fn log_insertion(&self, id: RecordId, fields: InsertionLogFields<'_>) {
        if let Err(e) = self.try_log_insertion(id, &fields) {
            eprintln!("parakit: insertion log write failed: {e:#}");
        }
    }

    fn try_log(
        &self,
        id: RecordId,
        audio_secs: f32,
        infer: Duration,
        raw: &str,
        cleaned: &str,
        rules_active: usize,
    ) -> Result<()> {
        let ts = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let record = LogRecord {
            ts,
            audio_secs,
            infer_ms: infer.as_millis(),
            raw,
            cleaned,
            rules_active,
        };

        match self.format {
            LogFormat::Jsonl => self.with_state(|state| {
                serde_json::to_writer(&mut state.file, &record)
                    .context("failed to serialize jsonl log record")?;
                writeln!(state.file).context("failed to write jsonl newline")?;
                state.file.flush().context("failed to flush log file")
            }),
            LogFormat::Tsv => {
                self.sweep_stale_pending_tsv_rows()?;
                let prefix = format!(
                    "{}\t{:.3}\t{}\t{}\t{}\t{}",
                    record.ts,
                    record.audio_secs,
                    record.infer_ms,
                    sanitize_tsv(record.raw),
                    sanitize_tsv(record.cleaned),
                    record.rules_active
                );
                self.pending_tsv.lock().insert(
                    id.0,
                    PendingTsvRow {
                        queued_at: Instant::now(),
                        prefix,
                    },
                );
                Ok(())
            }
        }
    }

    fn try_log_insertion(&self, id: RecordId, fields: &InsertionLogFields<'_>) -> Result<()> {
        match self.format {
            LogFormat::Jsonl => {
                let ts = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
                let record = InsertionLogRecord {
                    kind: "insertion",
                    ts,
                    ref_id: id.0,
                    outcome: fields.outcome,
                    target_bundle_id: fields.target_bundle_id,
                    focus_verification: fields.focus_verification,
                    transcript_chars: fields.transcript_chars,
                    paste_event_posted: fields.paste_event_posted,
                    pasteboard_requested: fields.pasteboard_requested,
                    acknowledgement_kind: fields.acknowledgement_kind,
                    acknowledgement_ms: fields.acknowledgement_ms,
                    clipboard_restored: fields.clipboard_restored,
                    failure_reason: fields.failure_reason,
                };
                self.with_state(|state| {
                    serde_json::to_writer(&mut state.file, &record)
                        .context("failed to serialize jsonl insertion record")?;
                    writeln!(state.file).context("failed to write jsonl newline")?;
                    state.file.flush().context("failed to flush log file")
                })
            }
            LogFormat::Tsv => {
                self.sweep_stale_pending_tsv_rows()?;
                let Some(pending) = self.pending_tsv.lock().remove(&id.0) else {
                    // The row already aged out of the pending map and was
                    // flushed with empty insertion columns by the sweep
                    // above; there is nothing left to complete for a TSV
                    // file's single-row-per-record layout.
                    return Ok(());
                };
                let line =
                    tsv_row_with_insertion_cells(&pending.prefix, &insertion_tsv_cells(fields));
                self.with_state(|state| {
                    writeln!(state.file, "{line}")
                        .context("failed to write tsv insertion record")?;
                    state.file.flush().context("failed to flush log file")
                })
            }
        }
    }

    /// Flush TSV rows that have waited longer than [`PENDING_TSV_MAX_AGE`]
    /// for their insertion outcome, so a dropped or unusually delayed
    /// [`DataLogger::log_insertion`] call never silently discards the
    /// transcript itself.
    fn sweep_stale_pending_tsv_rows(&self) -> Result<()> {
        let now = Instant::now();
        let stale: Vec<PendingTsvRow> = {
            let mut pending = self.pending_tsv.lock();
            let stale_ids: Vec<u64> = pending
                .iter()
                .filter(|(_, row)| {
                    now.saturating_duration_since(row.queued_at) >= PENDING_TSV_MAX_AGE
                })
                .map(|(id, _)| *id)
                .collect();
            stale_ids
                .into_iter()
                .filter_map(|id| pending.remove(&id))
                .collect()
        };
        if stale.is_empty() {
            return Ok(());
        }

        let empty_cells = empty_insertion_tsv_cells();
        self.with_state(|state| {
            for row in &stale {
                let line = tsv_row_with_insertion_cells(&row.prefix, &empty_cells);
                writeln!(state.file, "{line}")
                    .context("failed to write orphaned tsv log record")?;
            }
            state.file.flush().context("failed to flush log file")
        })
    }

    /// Run `f` against the current daily log file, rotating it first if the
    /// local date has changed since the last write.
    fn with_state<F>(&self, f: F) -> Result<()>
    where
        F: FnOnce(&mut LogState) -> Result<()>,
    {
        let local_date = Local::now().date_naive();
        let mut state = self.state.lock();
        if state.as_ref().map(|s| s.date) != Some(local_date) {
            *state = Some(LogState {
                date: local_date,
                file: self.open_for_date(local_date)?,
            });
        }
        let state = state
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("log file state was not initialized"))?;
        f(state)
    }

    fn open_for_date(&self, date: NaiveDate) -> Result<BufWriter<File>> {
        create_dir_all(&self.dir)
            .with_context(|| format!("failed to create log dir {}", self.dir.display()))?;
        let path = self.dir.join(file_name(date, self.format));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("failed to open log file {}", path.display()))?;
        Ok(BufWriter::new(file))
    }
}

fn file_name(date: NaiveDate, format: LogFormat) -> String {
    let ext = match format {
        LogFormat::Jsonl => "jsonl",
        LogFormat::Tsv => "tsv",
    };
    format!("parakit-{}.{}", date.format("%Y-%m-%d"), ext)
}

fn sanitize_tsv(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\t' | '\r' | '\n' => ' ',
            other => other,
        })
        .collect()
}

fn opt_bool_cell(value: Option<bool>) -> String {
    match value {
        Some(true) => "true".to_string(),
        Some(false) => "false".to_string(),
        None => String::new(),
    }
}

fn insertion_tsv_cells(fields: &InsertionLogFields<'_>) -> [String; INSERTION_TSV_COLUMNS] {
    [
        sanitize_tsv(fields.outcome),
        fields
            .target_bundle_id
            .map(sanitize_tsv)
            .unwrap_or_default(),
        sanitize_tsv(fields.focus_verification),
        fields.transcript_chars.to_string(),
        fields.paste_event_posted.to_string(),
        opt_bool_cell(fields.pasteboard_requested),
        sanitize_tsv(fields.acknowledgement_kind),
        fields
            .acknowledgement_ms
            .map(|ms| ms.to_string())
            .unwrap_or_default(),
        opt_bool_cell(fields.clipboard_restored),
        fields.failure_reason.map(sanitize_tsv).unwrap_or_default(),
    ]
}

fn empty_insertion_tsv_cells() -> [String; INSERTION_TSV_COLUMNS] {
    std::array::from_fn(|_| String::new())
}

fn tsv_row_with_insertion_cells(prefix: &str, cells: &[String; INSERTION_TSV_COLUMNS]) -> String {
    let mut line = String::from(prefix);
    for cell in cells {
        line.push('\t');
        line.push_str(cell);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn concurrent_jsonl_logging_writes_all_lines() {
        let dir = crate::test_support::fixture_root("parakit-log-test", "jsonl");
        let logger = Arc::new(DataLogger::new(dir.clone(), LogFormat::Jsonl));

        let mut threads = Vec::new();
        for thread_id in 0..10 {
            let logger = Arc::clone(&logger);
            threads.push(std::thread::spawn(move || {
                for i in 0..100 {
                    logger.log(
                        4.21,
                        Duration::from_millis(187),
                        &format!("raw {thread_id} {i}"),
                        &format!("cleaned {thread_id} {i}"),
                        72,
                    );
                }
            }));
        }

        for thread in threads {
            thread.join().expect("logging thread panicked");
        }

        let date = Local::now().date_naive();
        let path = dir.join(file_name(date, LogFormat::Jsonl));
        let contents = std::fs::read_to_string(&path).expect("read log file");
        assert_eq!(contents.lines().count(), 1000);
        for line in contents.lines() {
            let value: serde_json::Value = serde_json::from_str(line).expect("valid jsonl");
            assert_eq!(value["rules_active"], 72);
        }
    }

    #[test]
    fn tsv_sanitizes_tabs_and_newlines() {
        assert_eq!(sanitize_tsv("a\tb\nc\rd"), "a b c d");
    }

    fn sample_insertion_fields() -> InsertionLogFields<'static> {
        InsertionLogFields {
            outcome: "pasted",
            target_bundle_id: Some("com.example.App"),
            focus_verification: "not_applicable",
            transcript_chars: 12,
            paste_event_posted: true,
            pasteboard_requested: None,
            acknowledgement_kind: "not_applicable",
            acknowledgement_ms: Some(120),
            clipboard_restored: Some(true),
            failure_reason: None,
        }
    }

    #[test]
    fn jsonl_log_insertion_emits_correlated_second_line() {
        let dir = crate::test_support::fixture_root("parakit-log-test", "jsonl-insertion");
        let logger = DataLogger::new(dir.clone(), LogFormat::Jsonl);

        let id = logger.log(
            1.5,
            Duration::from_millis(42),
            "raw text",
            "cleaned text",
            3,
        );
        logger.log_insertion(id, sample_insertion_fields());

        let date = Local::now().date_naive();
        let path = dir.join(file_name(date, LogFormat::Jsonl));
        let contents = std::fs::read_to_string(&path).expect("read log file");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(
            lines.len(),
            2,
            "expected one transcript line and one insertion line"
        );

        let transcript: serde_json::Value =
            serde_json::from_str(lines[0]).expect("valid transcript jsonl");
        assert!(
            transcript.get("kind").is_none(),
            "transcript record must not gain a kind key"
        );
        assert_eq!(transcript["cleaned"], "cleaned text");

        let insertion: serde_json::Value =
            serde_json::from_str(lines[1]).expect("valid insertion jsonl");
        assert_eq!(insertion["kind"], "insertion");
        assert_eq!(insertion["ref_id"], id.0);
        assert_eq!(insertion["outcome"], "pasted");
        assert_eq!(insertion["target_bundle_id"], "com.example.App");
        assert_eq!(insertion["focus_verification"], "not_applicable");
        assert_eq!(insertion["paste_event_posted"], true);
        assert_eq!(insertion["acknowledgement_ms"], 120);
        assert_eq!(insertion["clipboard_restored"], true);
        assert_eq!(insertion["pasteboard_requested"], serde_json::Value::Null);
    }

    #[test]
    fn tsv_row_appears_only_after_log_insertion() {
        let dir = crate::test_support::fixture_root("parakit-log-test", "tsv-deferred");
        let logger = DataLogger::new(dir.clone(), LogFormat::Tsv);

        let id = logger.log(2.0, Duration::from_millis(88), "raw", "cleaned", 5);

        let date = Local::now().date_naive();
        let path = dir.join(file_name(date, LogFormat::Tsv));
        let before = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            before.is_empty(),
            "transcript row should be deferred until log_insertion completes it"
        );

        logger.log_insertion(id, sample_insertion_fields());

        let contents = std::fs::read_to_string(&path).expect("read tsv log file");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 1);
        let cols: Vec<&str> = lines[0].split('\t').collect();
        assert_eq!(cols.len(), 6 + INSERTION_TSV_COLUMNS);
        assert_eq!(cols[3], "raw");
        assert_eq!(cols[4], "cleaned");
        assert_eq!(cols[6], "pasted");
        assert_eq!(cols[7], "com.example.App");
        assert_eq!(cols[8], "not_applicable");
        assert_eq!(cols[9], "12");
        assert_eq!(cols[10], "true");
        assert_eq!(cols[11], "");
        assert_eq!(cols[12], "not_applicable");
        assert_eq!(cols[13], "120");
        assert_eq!(cols[14], "true");
        assert_eq!(cols[15], "");
    }

    #[test]
    fn tsv_sweep_flushes_stale_orphaned_row_with_empty_insertion_cells() {
        let dir = crate::test_support::fixture_root("parakit-log-test", "tsv-sweep");
        let logger = DataLogger::new(dir.clone(), LogFormat::Tsv);

        let orphan_id = logger.log(
            1.0,
            Duration::from_millis(10),
            "orphan raw",
            "orphan cleaned",
            1,
        );
        {
            let mut pending = logger.pending_tsv.lock();
            let row = pending
                .get_mut(&orphan_id.0)
                .expect("pending row should exist before the sweep");
            row.queued_at = Instant::now() - PENDING_TSV_MAX_AGE - Duration::from_secs(1);
        }

        // Triggers the sweep as a side effect; the orphan is stale enough to
        // be flushed before this second row is buffered.
        let _second_id = logger.log(
            2.0,
            Duration::from_millis(20),
            "second raw",
            "second cleaned",
            2,
        );

        let date = Local::now().date_naive();
        let path = dir.join(file_name(date, LogFormat::Tsv));
        let contents = std::fs::read_to_string(&path).expect("read tsv log file");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(
            lines.len(),
            1,
            "only the swept orphan row should be flushed so far"
        );

        let cols: Vec<&str> = lines[0].split('\t').collect();
        assert_eq!(cols.len(), 6 + INSERTION_TSV_COLUMNS);
        assert_eq!(cols[3], "orphan raw");
        assert_eq!(cols[4], "orphan cleaned");
        for col in &cols[6..] {
            assert!(
                col.is_empty(),
                "insertion columns should be empty for a swept orphan row"
            );
        }

        assert!(
            !logger.pending_tsv.lock().contains_key(&orphan_id.0),
            "swept row should be removed from pending state"
        );
    }
}
