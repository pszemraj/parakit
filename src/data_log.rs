//! Transcription logging for collecting raw/cleaned cleanup pairs.

use crate::rules::RuleHit;
use anyhow::{Context, Result};
use chrono::{Local, NaiveDate, SecondsFormat, Utc};
use parking_lot::Mutex;
use serde::Serialize;
use std::fs::{create_dir_all, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Identifier returned by [`DataLogger::log`] that correlates a
/// transcription record with its later insertion outcome recorded through
/// [`DataLogger::log_insertion`].
#[derive(Clone, Copy, Debug)]
pub struct RecordId(u64);

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
    /// Minimum isolated number converted to digits; None means convert all.
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
    /// `"unverified_timeout"`, `"no_evidence"`, or `"not_applicable"`.
    pub acknowledgement_kind: &'static str,
    /// Milliseconds spent waiting for acknowledgement, when applicable.
    pub acknowledgement_ms: Option<u128>,
    /// Whether the previous clipboard contents were restored after insertion.
    pub clipboard_restored: Option<bool>,
    /// Human-readable failure reason when insertion errored.
    pub failure_reason: Option<&'a str>,
}

#[derive(Serialize)]
struct InsertionLogRecord<'a> {
    kind: &'static str,
    ts: String,
    ref_id: u64,
    #[serde(flatten)]
    fields: InsertionLogFields<'a>,
}

struct LogState {
    date: NaiveDate,
    file: BufWriter<File>,
}

/// Synchronous JSONL transcription logger with lazy daily file rotation.
pub struct DataLogger {
    dir: PathBuf,
    state: Mutex<Option<LogState>>,
    next_id: AtomicU64,
}

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
    /// An identifier that correlates this record with its insertion outcome.
    pub fn log(
        &self,
        audio_secs: f32,
        infer: Duration,
        raw: &str,
        cleaned: &str,
        cleaning: CleaningLogFields<'_>,
    ) -> RecordId {
        let id = RecordId(self.next_id.fetch_add(1, Ordering::Relaxed));
        if let Err(e) = self.try_log(audio_secs, infer, raw, cleaned, cleaning) {
            eprintln!("parakit: transcription log write failed: {e:#}");
        }
        id
    }

    /// Write the insertion outcome for a record previously returned by
    /// [`DataLogger::log`].
    ///
    /// This appends a second, independently parseable JSONL line carrying
    /// `"kind":"insertion"` and `"ref_id"` set to the original record's
    /// identifier.
    ///
    /// Logging failures are printed to stderr and never propagated, for the
    /// same reason as [`DataLogger::log`].
    ///
    /// # Arguments
    ///
    /// * `id` - Identifier returned by the original [`DataLogger::log`] call.
    /// * `fields` - Insertion telemetry to record.
    pub fn log_insertion(&self, id: RecordId, fields: InsertionLogFields<'_>) {
        if let Err(e) = self.try_log_insertion(id, fields) {
            eprintln!("parakit: insertion log write failed: {e:#}");
        }
    }

    fn try_log(
        &self,
        audio_secs: f32,
        infer: Duration,
        raw: &str,
        cleaned: &str,
        cleaning: CleaningLogFields<'_>,
    ) -> Result<()> {
        let ts = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let record = LogRecord {
            ts,
            parakit_version: crate::build_info::PACKAGE_VERSION,
            audio_secs,
            infer_ms: infer.as_millis(),
            raw,
            cleaned,
            cleaning,
        };

        self.with_state(|state| {
            write_jsonl_record(state, &record, "failed to serialize jsonl log record")
        })
    }

    fn try_log_insertion(&self, id: RecordId, fields: InsertionLogFields<'_>) -> Result<()> {
        let ts = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
        let record = InsertionLogRecord {
            kind: "insertion",
            ts,
            ref_id: id.0,
            fields,
        };
        self.with_state(|state| {
            write_jsonl_record(state, &record, "failed to serialize jsonl insertion record")
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
            .expect("log state is initialized or open_for_date returned an error");
        f(state)
    }

    fn open_for_date(&self, date: NaiveDate) -> Result<BufWriter<File>> {
        create_dir_all(&self.dir)
            .with_context(|| format!("failed to create log dir {}", self.dir.display()))?;
        let path = self.dir.join(file_name(date));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("failed to open log file {}", path.display()))?;
        Ok(BufWriter::new(file))
    }
}

fn write_jsonl_record<T: Serialize>(
    state: &mut LogState,
    record: &T,
    serialization_context: &'static str,
) -> Result<()> {
    serde_json::to_writer(&mut state.file, record).context(serialization_context)?;
    writeln!(state.file).context("failed to write jsonl newline")?;
    state.file.flush().context("failed to flush log file")
}

fn file_name(date: NaiveDate) -> String {
    format!("parakit-{}.jsonl", date.format("%Y-%m-%d"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn concurrent_jsonl_logging_writes_all_lines() {
        let dir = crate::test_support::fixture_root("parakit-log-test", "jsonl");
        let logger = Arc::new(DataLogger::new(dir.clone()));

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
                        CleaningLogFields {
                            rules_active: 72,
                            ..sample_cleaning_fields()
                        },
                    );
                }
            }));
        }

        for thread in threads {
            thread.join().expect("logging thread panicked");
        }

        let date = Local::now().date_naive();
        let path = dir.join(file_name(date));
        let contents = std::fs::read_to_string(&path).expect("read log file");
        assert_eq!(contents.lines().count(), 1000);
        for line in contents.lines() {
            let value: serde_json::Value = serde_json::from_str(line).expect("valid jsonl");
            assert_eq!(value["rules_active"], 72);
            assert_eq!(value["parakit_version"], crate::build_info::PACKAGE_VERSION);
        }
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

    fn sample_cleaning_fields() -> CleaningLogFields<'static> {
        CleaningLogFields {
            rules_active: 3,
            cleaner_version: 1,
            profile: "safe",
            ruleset_id: Some("safe-v1"),
            drops_trailing_period: true,
            number_threshold: None,
            rules_fired: &[],
            failure: None,
        }
    }

    fn assert_exact_keys(value: &serde_json::Value, expected: &[&str]) {
        let object = value.as_object().expect("record should be a JSON object");
        assert_eq!(object.len(), expected.len(), "unexpected keys in {value}");
        for key in expected {
            assert!(object.contains_key(*key), "missing {key:?} in {value}");
        }
    }

    #[test]
    fn jsonl_log_insertion_emits_correlated_second_line() {
        let dir = crate::test_support::fixture_root("parakit-log-test", "jsonl-insertion");
        let logger = DataLogger::new(dir.clone());

        let id = logger.log(
            1.5,
            Duration::from_millis(42),
            "raw text",
            "cleaned text",
            sample_cleaning_fields(),
        );
        logger.log_insertion(id, sample_insertion_fields());

        let date = Local::now().date_naive();
        let path = dir.join(file_name(date));
        let contents = std::fs::read_to_string(&path).expect("read log file");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(
            lines.len(),
            2,
            "expected one transcript line and one insertion line"
        );

        let transcript: serde_json::Value =
            serde_json::from_str(lines[0]).expect("valid transcript jsonl");
        assert_exact_keys(
            &transcript,
            &[
                "ts",
                "parakit_version",
                "audio_secs",
                "infer_ms",
                "raw",
                "cleaned",
                "rules_active",
                "cleaner_version",
                "cleaning_profile",
                "ruleset_id",
                "drops_trailing_period",
                "number_threshold",
                "rules_fired",
            ],
        );
        assert_eq!(transcript["cleaned"], "cleaned text");
        assert_eq!(
            transcript["parakit_version"],
            crate::build_info::PACKAGE_VERSION
        );

        let insertion: serde_json::Value =
            serde_json::from_str(lines[1]).expect("valid insertion jsonl");
        assert_exact_keys(
            &insertion,
            &[
                "kind",
                "ts",
                "ref_id",
                "outcome",
                "target_bundle_id",
                "focus_verification",
                "transcript_chars",
                "paste_event_posted",
                "pasteboard_requested",
                "acknowledgement_kind",
                "acknowledgement_ms",
                "clipboard_restored",
                "failure_reason",
            ],
        );
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
    fn jsonl_log_includes_cleaning_fields() {
        let dir = crate::test_support::fixture_root("parakit-log-test", "jsonl-cleaning");
        let logger = DataLogger::new(dir.clone());

        let hits = vec![
            RuleHit {
                name: "trailing_period".to_string(),
                matches: 2,
            },
            RuleHit {
                name: "filler_words".to_string(),
                matches: 5,
            },
        ];
        let fields = CleaningLogFields {
            rules_active: 4,
            cleaner_version: 7,
            profile: "aggressive",
            ruleset_id: Some("aggressive-v7"),
            drops_trailing_period: true,
            number_threshold: Some(5.0),
            rules_fired: &hits,
            failure: None,
        };
        logger.log(1.0, Duration::from_millis(10), "raw", "cleaned", fields);

        let date = Local::now().date_naive();
        let path = dir.join(file_name(date));
        let contents = std::fs::read_to_string(&path).expect("read log file");
        let value: serde_json::Value =
            serde_json::from_str(contents.lines().next().expect("one line")).expect("valid jsonl");

        assert_eq!(value["rules_active"], 4);
        assert_eq!(value["parakit_version"], crate::build_info::PACKAGE_VERSION);
        assert_eq!(value["cleaner_version"], 7);
        assert_eq!(value["cleaning_profile"], "aggressive");
        assert_eq!(value["ruleset_id"], "aggressive-v7");
        assert_eq!(value["drops_trailing_period"], true);
        assert_eq!(value["number_threshold"], 5.0);
        assert_eq!(
            value["rules_fired"],
            serde_json::json!([
                {"name": "trailing_period", "matches": 2},
                {"name": "filler_words", "matches": 5},
            ])
        );
        assert!(
            value.get("cleaning_failure").is_none(),
            "cleaning_failure should be omitted when there was no failure"
        );
    }

    #[test]
    fn jsonl_log_omits_optional_cleaning_fields_when_absent() {
        let dir = crate::test_support::fixture_root("parakit-log-test", "jsonl-cleaning-quiet");
        let logger = DataLogger::new(dir.clone());

        let fields = CleaningLogFields {
            rules_active: 0,
            cleaner_version: 1,
            profile: "disabled",
            ruleset_id: None,
            drops_trailing_period: false,
            number_threshold: None,
            rules_fired: &[],
            failure: None,
        };
        logger.log(1.0, Duration::from_millis(5), "raw", "raw", fields);

        let date = Local::now().date_naive();
        let path = dir.join(file_name(date));
        let contents = std::fs::read_to_string(&path).expect("read log file");
        let value: serde_json::Value =
            serde_json::from_str(contents.lines().next().expect("one line")).expect("valid jsonl");

        assert!(
            value.get("ruleset_id").is_none(),
            "ruleset_id should be omitted when None"
        );
        assert!(
            value.get("cleaning_failure").is_none(),
            "cleaning_failure should be omitted when None"
        );
        assert_eq!(value["cleaning_profile"], "disabled");
        assert_eq!(value["number_threshold"], serde_json::Value::Null);
        assert_eq!(value["rules_fired"], serde_json::json!([]));
    }

    #[test]
    fn cleaning_failure_is_recorded_in_jsonl() {
        let dir = crate::test_support::fixture_root("parakit-log-test", "jsonl-cleaning-failure");
        let logger = DataLogger::new(dir.clone());

        let fields = CleaningLogFields {
            failure: Some("panic: rule 'foo' bar"),
            ..sample_cleaning_fields()
        };
        logger.log(1.0, Duration::from_millis(5), "raw", "raw", fields);

        let date = Local::now().date_naive();
        let path = dir.join(file_name(date));
        let contents = std::fs::read_to_string(&path).expect("read log file");
        let value: serde_json::Value =
            serde_json::from_str(contents.lines().next().expect("one line")).expect("valid jsonl");

        assert_eq!(value["cleaning_failure"], "panic: rule 'foo' bar");
    }
}
