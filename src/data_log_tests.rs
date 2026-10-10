//! JSONL transaction and telemetry regressions.

use super::*;
use std::sync::Arc;

#[test]
fn concurrent_jsonl_logging_writes_all_lines() {
    let dir = crate::test_support::fixture_root("parakit-log-test", "jsonl");
    let logger = Arc::new(DataLogger::new(dir.clone()));

    let mut threads = Vec::new();
    for thread_id in 0..10 {
        let logger = if thread_id < 5 {
            Arc::clone(&logger)
        } else {
            Arc::new(DataLogger::new(dir.clone()))
        };
        threads.push(std::thread::spawn(move || {
            for i in 0..100 {
                logger
                    .log(
                        4.21,
                        Duration::from_millis(187),
                        &format!("raw {thread_id} {i}"),
                        &format!("cleaned {thread_id} {i}"),
                        CleaningLogFields {
                            rules_active: 72,
                            ..sample_cleaning_fields()
                        },
                    )
                    .expect("write concurrent log record");
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

#[test]
fn jsonl_round_trips_adversarial_transcript_content() {
    let dir = crate::test_support::fixture_root("parakit-log-test", "adversarial-content");
    let logger = DataLogger::new(dir.clone());
    let raw = "quote: \"; slash: \\; newline:\n; controls:\u{0000}\u{001f}";
    let cleaned = "cleaned\r\n\t\"value\"";

    logger
        .log(
            1.0,
            Duration::from_millis(10),
            raw,
            cleaned,
            sample_cleaning_fields(),
        )
        .expect("write adversarial log record");

    let path = dir.join(file_name(Local::now().date_naive()));
    let contents = std::fs::read_to_string(path).expect("read adversarial log record");
    assert_eq!(contents.lines().count(), 1);
    let value: serde_json::Value =
        serde_json::from_str(contents.trim_end()).expect("valid JSONL record");
    assert_eq!(value["raw"], raw);
    assert_eq!(value["cleaned"], cleaned);
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

    let id = logger
        .log(
            1.5,
            Duration::from_millis(42),
            "raw text",
            "cleaned text",
            sample_cleaning_fields(),
        )
        .expect("write transcript record");
    logger.log_insertion(&id, sample_insertion_fields());

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
            "session_id",
            "record_id",
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
    assert_eq!(transcript["session_id"], id.session_id.as_ref());
    assert_eq!(transcript["record_id"], id.sequence);
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
            "session_id",
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
    assert_eq!(insertion["session_id"], id.session_id.as_ref());
    assert_eq!(insertion["ref_id"], id.sequence);
    assert_eq!(insertion["outcome"], "pasted");
    assert_eq!(insertion["target_bundle_id"], "com.example.App");
    assert_eq!(insertion["focus_verification"], "not_applicable");
    assert_eq!(insertion["paste_event_posted"], true);
    assert_eq!(insertion["acknowledgement_ms"], 120);
    assert_eq!(insertion["clipboard_restored"], true);
    assert_eq!(insertion["pasteboard_requested"], serde_json::Value::Null);
}

#[test]
fn logger_sessions_disambiguate_sequence_restarts_in_one_daily_file() {
    let dir = crate::test_support::fixture_root("parakit-log-test", "session-correlation");

    let first_logger = DataLogger::new(dir.clone());
    let first_id = first_logger
        .log(
            1.0,
            Duration::from_millis(10),
            "first raw",
            "first cleaned",
            sample_cleaning_fields(),
        )
        .expect("write first-session transcript");
    first_logger.log_insertion(&first_id, sample_insertion_fields());
    drop(first_logger);

    let second_logger = DataLogger::new(dir.clone());
    let second_id = second_logger
        .log(
            1.0,
            Duration::from_millis(10),
            "second raw",
            "second cleaned",
            sample_cleaning_fields(),
        )
        .expect("write second-session transcript");
    second_logger.log_insertion(&second_id, sample_insertion_fields());

    let path = dir.join(file_name(Local::now().date_naive()));
    let records: Vec<serde_json::Value> = std::fs::read_to_string(path)
        .expect("read shared daily log")
        .lines()
        .map(|line| serde_json::from_str(line).expect("valid JSONL record"))
        .collect();
    assert_eq!(records.len(), 4);

    assert_eq!(records[0]["record_id"], 0);
    assert_eq!(records[1]["ref_id"], 0);
    assert_eq!(records[2]["record_id"], 0);
    assert_eq!(records[3]["ref_id"], 0);
    assert_eq!(records[0]["session_id"], records[1]["session_id"]);
    assert_eq!(records[2]["session_id"], records[3]["session_id"]);
    assert_ne!(records[0]["session_id"], records[2]["session_id"]);
}

#[test]
fn insertion_record_stays_in_the_transcriptions_daily_file() {
    let dir = crate::test_support::fixture_root("parakit-log-test", "insertion-rotation");
    let logger = DataLogger::new(dir.clone());
    let first_date = NaiveDate::from_ymd_opt(2026, 1, 31).expect("valid date");
    let next_date = NaiveDate::from_ymd_opt(2026, 2, 1).expect("valid date");
    let first_id = RecordId {
        session_id: Arc::clone(&logger.session_id),
        sequence: 41,
        local_date: first_date,
    };
    let next_id = RecordId {
        session_id: Arc::clone(&logger.session_id),
        sequence: 42,
        local_date: next_date,
    };

    logger
        .try_log(
            LogTimestamp {
                record_id: first_id.clone(),
                utc_rfc3339: "2026-02-01T04:59:59.900Z".to_string(),
            },
            1.0,
            Duration::from_millis(10),
            "first raw",
            "first cleaned",
            sample_cleaning_fields(),
        )
        .expect("write first-day transcript");
    logger
        .try_log(
            LogTimestamp {
                record_id: next_id,
                utc_rfc3339: "2026-02-01T05:00:00.100Z".to_string(),
            },
            1.0,
            Duration::from_millis(10),
            "next raw",
            "next cleaned",
            sample_cleaning_fields(),
        )
        .expect("rotate to next-day transcript");

    logger.log_insertion(&first_id, sample_insertion_fields());

    let first_contents =
        std::fs::read_to_string(dir.join(file_name(first_date))).expect("read first-day log");
    let next_contents =
        std::fs::read_to_string(dir.join(file_name(next_date))).expect("read next-day log");
    let first_lines: Vec<_> = first_contents.lines().collect();
    assert_eq!(first_lines.len(), 2);
    assert_eq!(next_contents.lines().count(), 1);

    let insertion: serde_json::Value =
        serde_json::from_str(first_lines[1]).expect("valid insertion JSON");
    assert_eq!(insertion["kind"], "insertion");
    assert_eq!(insertion["ref_id"], first_id.sequence);
}

#[test]
fn failed_transcription_write_returns_no_record_id() {
    let root = crate::test_support::fixture_root("parakit-log-test", "write-failure");
    let blocked_dir = root.join("not-a-directory");
    std::fs::write(&blocked_dir, b"file blocks log directory")
        .expect("create log-directory blocker");
    let logger = DataLogger::new(blocked_dir);

    let id = logger.log(
        1.0,
        Duration::from_millis(10),
        "raw",
        "cleaned",
        sample_cleaning_fields(),
    );

    assert!(id.is_none());
}

#[test]
fn failed_record_write_discards_log_state() {
    let dir = crate::test_support::fixture_root("parakit-log-test", "state-reset");
    let logger = DataLogger::new(dir);
    let date = Local::now().date_naive();

    let result = logger.with_state(date, |_state| anyhow::bail!("injected write failure"));

    assert!(result.is_err());
    assert!(logger.state.lock().is_none());
}

#[test]
fn failed_partial_append_restores_the_previous_file() {
    let dir = crate::test_support::fixture_root("parakit-log-test", "partial-write-rollback");
    let date = Local::now().date_naive();
    let path = dir.join(file_name(date));
    std::fs::write(&path, b"{\"valid\":true}\n").expect("write existing record");
    let logger = DataLogger::new(dir);
    let mut file = logger.open_for_date(date).expect("open rollback fixture");

    let result = append_with_rollback(&mut file, |file| {
        file.write_all(b"{\"partial\":")?;
        Err(std::io::Error::other("injected write failure"))
    });

    assert_eq!(
        result.expect_err("injected append should fail").to_string(),
        "injected write failure"
    );
    assert_eq!(
        std::fs::read_to_string(&path).expect("read rolled-back fixture"),
        "{\"valid\":true}\n"
    );
    append_with_rollback(&mut file, |file| file.write_all(b"{\"next\":true}\n"))
        .expect("append after rollback");
    drop(file);
    assert_eq!(
        std::fs::read_to_string(&path).expect("read rolled-back fixture"),
        "{\"valid\":true}\n{\"next\":true}\n"
    );
    // A failed rollback must also release the lock for another handle.
    let mut read_only = File::open(&path).unwrap();
    assert!(append_with_rollback(&mut read_only, |file| file.write_all(b"bad")).is_err());
    let other = logger.open_for_date(date).unwrap();
    FileExt::try_lock_exclusive(&other).expect("release lock after failed rollback");
    FileExt::unlock(&other).unwrap();
}

#[test]
fn independent_writable_handles_serialize_append_and_rollback() {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};
    use std::process::{Command, Stdio};
    use std::sync::mpsc::channel;

    const CHILD_PATH: &str = "PARAKIT_TEST_LOG_APPEND_PATH";
    if let Some(path) = std::env::var_os(CHILD_PATH) {
        let mut file = OpenOptions::new().write(true).open(path).unwrap();
        // Seek before the first process finishes: the transaction must refresh
        // this stale offset after acquiring its cross-process lock.
        file.seek(SeekFrom::End(0)).unwrap();
        assert!(FileExt::try_lock_exclusive(&file).is_err());
        println!("logger-child-ready");
        std::io::stdout().flush().unwrap();
        append_with_rollback(&mut file, |file| file.write_all(b"{\"second\":true}\n")).unwrap();
        return;
    }

    for fail_first in [true, false] {
        let dir = crate::test_support::fixture_root(
            "parakit-log-test",
            &format!("writable-append-{fail_first}"),
        );
        create_dir_all(&dir).unwrap();
        let path = dir.join("shared.jsonl");
        // Exercise Windows-style writable handles on every platform.
        let open = || {
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(&path)
                .unwrap()
        };
        let mut first = open();
        let (entered_tx, entered_rx) = channel();
        let (release_tx, release_rx) = channel();
        let first_writer = std::thread::spawn(move || {
            append_with_rollback(&mut first, |file| {
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                file.write_all(b"{\"first\":true}\n")?;
                if fail_first {
                    return Err(std::io::Error::other("injected write failure"));
                }
                Ok(())
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let mut second_writer = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "data_log::tests::independent_writable_handles_serialize_append_and_rollback",
                "--nocapture",
            ])
            .env(CHILD_PATH, &path)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = BufReader::new(second_writer.stdout.take().unwrap());
        let mut ready = false;
        loop {
            let mut line = String::new();
            if output.read_line(&mut line).unwrap() == 0 {
                break;
            }
            if line.trim_end() == "logger-child-ready" {
                ready = true;
                break;
            }
        }
        let wrote_during_first = second_writer.try_wait().unwrap().is_some();
        release_tx.send(()).unwrap();
        let first_result = first_writer.join().unwrap();
        assert!(second_writer.wait().unwrap().success());
        assert!(ready, "child must encounter the first process's held lock");
        assert_eq!(first_result.is_err(), fail_first);
        let expected = if fail_first {
            "{\"second\":true}\n"
        } else {
            "{\"first\":true}\n{\"second\":true}\n"
        };
        assert_eq!(std::fs::read_to_string(path).unwrap(), expected);
        assert!(
            !wrote_during_first,
            "another handle wrote before append/rollback finished"
        );
    }
}

#[cfg(unix)]
#[test]
fn independent_handles_append_after_another_logger_writes() {
    let dir = crate::test_support::fixture_root("parakit-log-test", "independent-append");
    let date = Local::now().date_naive();
    let path = dir.join(file_name(date));
    let logger = DataLogger::new(dir);
    let mut first = logger
        .open_for_date(date)
        .expect("open first logger handle");
    let mut second = logger
        .open_for_date(date)
        .expect("open second logger handle");

    append_with_rollback(&mut first, |first| {
        // Unix append positioning still protects against an unrelated writer
        // that does not take the logger's advisory lock.
        second.write_all(b"{\"second\":true}\n")?;
        first.write_all(b"{\"first\":true}\n")
    })
    .expect("append both records");

    assert_eq!(
        std::fs::read_to_string(path).expect("read shared daily log"),
        "{\"second\":true}\n{\"first\":true}\n"
    );
}

/// Expected shape of one optional-vs-nullable field in the JSON record.
///
/// `ruleset_id` and `cleaning_failure` use
/// `#[serde(skip_serializing_if = "Option::is_none")]`, so a `None` value
/// is OMITTED from the JSON entirely. `number_threshold` has no such
/// attribute, so a `None` value is ALWAYS PRESENT, serialized as JSON
/// `null`. There is deliberately no bare-null catch-all variant here: a
/// row must say which of the two contracts it expects for each field.
enum Field {
    Omitted,
    Value(serde_json::Value),
}

fn assert_field(value: &serde_json::Value, key: &str, expect: &Field, label: &str) {
    match expect {
        Field::Omitted => assert!(
            value.get(key).is_none(),
            "{label}: {key} should be omitted, got {:?}",
            value.get(key)
        ),
        Field::Value(expected) => {
            let actual = value
                .get(key)
                .unwrap_or_else(|| panic!("{label}: {key} should be present"));
            assert_eq!(actual, expected, "{label}: {key}");
        }
    }
}

/// One row of [`cleaning_log_fields_serialize_with_omit_vs_null_contract`].
struct CleaningLogCase<'a> {
    label: &'static str,
    fields: CleaningLogFields<'a>,
    expect_ruleset_id: Field,
    expect_number_threshold: Field,
    expect_rules_fired: serde_json::Value,
    expect_cleaning_failure: Field,
    expect_cleaner_version: serde_json::Value,
    expect_cleaning_profile: serde_json::Value,
    expect_drops_trailing_period: serde_json::Value,
}

#[test]
fn cleaning_log_fields_serialize_with_omit_vs_null_contract() {
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

    let cases = vec![
        CleaningLogCase {
            label: "ruleset_id and number_threshold both present",
            fields: CleaningLogFields {
                rules_active: 4,
                cleaner_version: 7,
                profile: "aggressive",
                ruleset_id: Some("aggressive-v7"),
                drops_trailing_period: true,
                number_threshold: Some(5.0),
                rules_fired: &hits,
                failure: None,
            },
            expect_ruleset_id: Field::Value(serde_json::json!("aggressive-v7")),
            expect_number_threshold: Field::Value(serde_json::json!(5.0)),
            expect_rules_fired: serde_json::json!([
                {"name": "trailing_period", "matches": 2},
                {"name": "filler_words", "matches": 5},
            ]),
            expect_cleaning_failure: Field::Omitted,
            expect_cleaner_version: serde_json::json!(7),
            expect_cleaning_profile: serde_json::json!("aggressive"),
            expect_drops_trailing_period: serde_json::json!(true),
        },
        CleaningLogCase {
            label: "ruleset_id omitted, number_threshold null, cleaning_failure omitted",
            fields: CleaningLogFields {
                rules_active: 0,
                cleaner_version: 1,
                profile: "disabled",
                ruleset_id: None,
                drops_trailing_period: false,
                number_threshold: None,
                rules_fired: &[],
                failure: None,
            },
            expect_ruleset_id: Field::Omitted,
            expect_number_threshold: Field::Value(serde_json::Value::Null),
            expect_rules_fired: serde_json::json!([]),
            expect_cleaning_failure: Field::Omitted,
            expect_cleaner_version: serde_json::json!(1),
            expect_cleaning_profile: serde_json::json!("disabled"),
            expect_drops_trailing_period: serde_json::json!(false),
        },
        CleaningLogCase {
            label: "cleaning_failure is recorded as a present value",
            fields: CleaningLogFields {
                failure: Some("panic: rule 'foo' bar"),
                ..sample_cleaning_fields()
            },
            // sample_cleaning_fields() sets ruleset_id: Some("safe-v1")
            // and number_threshold: None; asserting both here (not just
            // cleaning_failure, which is all the replaced test checked)
            // is a deliberate uniform-coverage increase.
            expect_ruleset_id: Field::Value(serde_json::json!("safe-v1")),
            expect_number_threshold: Field::Value(serde_json::Value::Null),
            expect_rules_fired: serde_json::json!([]),
            expect_cleaning_failure: Field::Value(serde_json::json!("panic: rule 'foo' bar")),
            expect_cleaner_version: serde_json::json!(1),
            expect_cleaning_profile: serde_json::json!("safe"),
            expect_drops_trailing_period: serde_json::json!(true),
        },
    ];

    for case in cases {
        let value = serde_json::to_value(case.fields)
            .unwrap_or_else(|e| panic!("{}: serialize cleaning fields: {e}", case.label));

        assert_field(&value, "ruleset_id", &case.expect_ruleset_id, case.label);
        assert_field(
            &value,
            "number_threshold",
            &case.expect_number_threshold,
            case.label,
        );
        assert_field(
            &value,
            "cleaning_failure",
            &case.expect_cleaning_failure,
            case.label,
        );
        assert_eq!(
            value["rules_fired"], case.expect_rules_fired,
            "{}: rules_fired",
            case.label
        );
        assert_eq!(
            value["cleaner_version"], case.expect_cleaner_version,
            "{}: cleaner_version",
            case.label
        );
        assert_eq!(
            value["cleaning_profile"], case.expect_cleaning_profile,
            "{}: cleaning_profile",
            case.label
        );
        assert_eq!(
            value["drops_trailing_period"], case.expect_drops_trailing_period,
            "{}: drops_trailing_period",
            case.label
        );
    }
}
