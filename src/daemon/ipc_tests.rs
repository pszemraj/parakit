//! IPC command policy, stop finalization, shared-state history, and wire-format regressions.

use super::*;

#[cfg(any(unix, target_os = "windows"))]
#[test]
fn daemon_not_running_disposition_covers_every_control_command() {
    let cases = [
        (
            IpcCommand::Status,
            DaemonNotRunningDisposition::Success("parakit: not running"),
        ),
        (
            IpcCommand::Stop,
            DaemonNotRunningDisposition::Success("parakit: not running; nothing to stop"),
        ),
        (
            IpcCommand::CopyLast { index: 0 },
            DaemonNotRunningDisposition::Error(
                "daemon is not running; start it with `parakit start` first",
            ),
        ),
        (
            IpcCommand::History { limit: None },
            DaemonNotRunningDisposition::Error(
                "daemon is not running; start it with `parakit start` first",
            ),
        ),
        (
            IpcCommand::TestPaste {
                text: "test".to_string(),
            },
            DaemonNotRunningDisposition::Error(
                "daemon is not running; start it with `parakit start` first",
            ),
        ),
    ];

    for (command, expected) in cases {
        assert_eq!(daemon_not_running_disposition(&command, false), expected);
    }
    let held = DaemonNotRunningDisposition::Error(
        "daemon control endpoint is unavailable while the singleton lock is held; it may still be starting or stopping",
    );
    assert_eq!(
        daemon_not_running_disposition(&IpcCommand::Status, true),
        held
    );
    assert_eq!(
        daemon_not_running_disposition(&IpcCommand::Stop, true),
        DaemonNotRunningDisposition::WaitForStop
    );
}

#[cfg(any(unix, target_os = "windows"))]
#[test]
fn stop_waits_for_a_held_lock_and_reports_a_persistent_holder() {
    let mut probes = 0;
    wait_for_daemon_stop_with_probe(Instant::now() + Duration::from_secs(1), || {
        probes += 1;
        Ok(probes < 3)
    })
    .expect("stop should wait until the lock is released");
    assert_eq!(probes, 3);

    let error = wait_for_daemon_stop_with_probe(Instant::now(), || Ok(true))
        .expect_err("a persistent lock holder should time out");
    assert!(error.to_string().contains("if this persists"));
    #[cfg(not(target_os = "windows"))]
    assert!(error.to_string().contains("pgrep -af parakit"));
    #[cfg(target_os = "windows")]
    assert!(error.to_string().contains("Get-Process parakit"));
}

#[cfg(any(unix, target_os = "windows"))]
#[test]
fn stop_success_message_describes_the_completed_state() {
    assert_eq!(
        completed_command_message(&IpcCommand::Stop, "stopping"),
        "stopped"
    );
    assert_eq!(
        completed_command_message(&IpcCommand::CopyLast { index: 0 }, "copied"),
        "copied"
    );
}

#[cfg(any(unix, target_os = "windows"))]
#[test]
fn ipc_command_response_timeout_matrix() {
    /// Expected `IpcCommand::response_timeout()` for `command`.
    ///
    /// Exhaustive with no wildcard arm: a new `IpcCommand` variant fails
    /// compilation here until this function states its response-timeout
    /// budget. This also closes a real coverage gap: `Stop` and
    /// `History` were never asserted before this matrix.
    fn expected_response_timeout(command: &IpcCommand) -> Duration {
        match command {
            IpcCommand::CopyLast { .. } | IpcCommand::TestPaste { .. } => {
                IPC_INSERT_RESPONSE_TIMEOUT
            }
            IpcCommand::Status | IpcCommand::Stop | IpcCommand::History { .. } => {
                IPC_TRANSPORT_TIMEOUT
            }
        }
    }

    let commands = [
        IpcCommand::Status,
        IpcCommand::Stop,
        IpcCommand::History { limit: None },
        IpcCommand::CopyLast { index: 0 },
        IpcCommand::TestPaste {
            text: "test".to_string(),
        },
    ];

    let failures: Vec<String> = commands
        .iter()
        .filter_map(|command| {
            let expected_timeout = expected_response_timeout(command);
            let actual_timeout = command.response_timeout();
            if actual_timeout != expected_timeout {
                return Some(format!(
                    "{command:?} timeout: expected {expected_timeout:?}, got {actual_timeout:?}"
                ));
            }

            None
        })
        .collect();

    assert!(
        failures.is_empty(),
        "{} case(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[cfg(any(unix, target_os = "windows"))]
#[test]
fn stop_finalization_waits_for_in_flight_insertion_and_excludes_new_insertions() {
    use std::sync::mpsc;

    let state = Arc::new(SharedState::new());
    let (insertion_started_tx, insertion_started_rx) = mpsc::channel();
    let (release_insertion_tx, release_insertion_rx) = mpsc::channel();
    let insertion_state = Arc::clone(&state);
    let insertion = thread::spawn(move || {
        insertion_state.with_insertion_lock(|| {
            insertion_started_tx
                .send(())
                .expect("test should observe in-flight insertion");
            release_insertion_rx
                .recv()
                .expect("test should release in-flight insertion");
        });
    });
    insertion_started_rx
        .recv()
        .expect("insertion should acquire the lock");

    let (stop_started_tx, stop_started_rx) = mpsc::channel();
    let (stop_has_lock_tx, stop_has_lock_rx) = mpsc::channel();
    let (finish_stop_tx, finish_stop_rx) = mpsc::channel();
    let stop_state = Arc::clone(&state);
    let stop = thread::spawn(move || {
        stop_started_tx
            .send(())
            .expect("test should observe stop finalization start");
        finish_stop_after_response(&stop_state, None, |graceful| {
            assert!(graceful);
            stop_has_lock_tx
                .send(())
                .expect("test should observe quiescent stop");
            finish_stop_rx
                .recv()
                .expect("test should release stop finalization");
        });
    });
    stop_started_rx
        .recv()
        .expect("stop finalization should start");
    assert!(
        stop_has_lock_rx
            .recv_timeout(STOP_RESPONSE_GRACE * 2)
            .is_err(),
        "stop must not terminate while an insertion holds the lock"
    );

    release_insertion_tx
        .send(())
        .expect("in-flight insertion should be released");
    stop_has_lock_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("stop should proceed after the insertion completes");

    let (later_insertion_tx, later_insertion_rx) = mpsc::channel();
    let later_state = Arc::clone(&state);
    let later_insertion = thread::spawn(move || {
        later_state.with_insertion_lock(|| {
            later_insertion_tx
                .send(())
                .expect("test should observe later insertion");
        });
    });
    assert!(
        later_insertion_rx
            .recv_timeout(STOP_RESPONSE_GRACE * 2)
            .is_err(),
        "no insertion may start once stop finalization owns the lock"
    );

    finish_stop_tx
        .send(())
        .expect("stop finalization should be released");
    stop.join().expect("stop finalization should return");
    insertion.join().expect("in-flight insertion should return");
    later_insertion_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("test replacement for process exit should release the lock");
    later_insertion
        .join()
        .expect("later insertion should return");
}

#[cfg(any(unix, target_os = "windows"))]
#[test]
fn stop_waits_for_worker_without_holding_its_insertion_lock() {
    let state = Arc::new(SharedState::new());
    let lifetime = state.shutdown.register();
    let worker_state = Arc::clone(&state);
    let worker = thread::spawn(move || {
        worker_state.activity.changes().recv().unwrap();
        assert!(worker_state.shutdown.requested());
        worker_state.with_insertion_lock(|| {});
        drop(lifetime);
    });
    finish_stop_after_response_with_wait(&state, None, Duration::from_secs(2), |graceful| {
        assert!(graceful, "worker must be able to finish before termination");
    });
    worker.join().unwrap();
}

#[cfg(any(unix, target_os = "windows"))]
#[test]
fn stop_finalization_forces_termination_when_insertion_is_wedged() {
    use std::sync::mpsc;

    let state = Arc::new(SharedState::new());
    let (insertion_started_tx, insertion_started_rx) = mpsc::channel();
    let (release_insertion_tx, release_insertion_rx) = mpsc::channel();
    let insertion_state = Arc::clone(&state);
    let insertion = thread::spawn(move || {
        insertion_state.with_insertion_lock(|| {
            insertion_started_tx
                .send(())
                .expect("test should observe in-flight insertion");
            release_insertion_rx
                .recv()
                .expect("test should release in-flight insertion");
        });
    });
    insertion_started_rx
        .recv()
        .expect("insertion should acquire the lock");

    let started = Instant::now();
    let terminated =
        finish_stop_after_response_with_wait(&state, None, Duration::from_millis(20), |graceful| {
            assert!(!graceful);
            true
        });

    assert!(terminated);
    assert!(started.elapsed() < Duration::from_secs(1));

    release_insertion_tx
        .send(())
        .expect("in-flight insertion should be released");
    insertion.join().expect("in-flight insertion should return");
}

#[test]
fn pipe_message_limit_rejects_oversize_before_buffering_it() {
    assert!(pipe_message_fits_limit(0, IPC_MAX_MESSAGE_SIZE, true));
    assert!(!pipe_message_fits_limit(0, IPC_MAX_MESSAGE_SIZE, false));
    assert!(!pipe_message_fits_limit(IPC_MAX_MESSAGE_SIZE, 1, true));
    assert!(!pipe_message_fits_limit(usize::MAX, 1, true));
}

#[test]
fn maximum_history_response_fits_message_limit() {
    let worst_case_preview = preview_text(&"\0".repeat(HISTORY_PREVIEW_MAX_CHARS + 1));
    let entries = (1..=MAX_TRANSCRIPT_HISTORY)
        .map(|index| HistoryEntry {
            index,
            age_secs: u64::MAX,
            chars: usize::MAX,
            preview: worst_case_preview.clone(),
        })
        .collect();
    let response = IpcResponse::History { entries };
    let encoded = serde_json::to_vec(&response).expect("history response should serialize");

    assert!(
        encoded.len() <= IPC_MAX_MESSAGE_SIZE,
        "maximum history response is {} bytes, limit is {IPC_MAX_MESSAGE_SIZE}",
        encoded.len()
    );
}

#[cfg(target_os = "macos")]
#[test]
fn insertion_response_timeout_covers_queued_macos_paste_transaction() {
    let acknowledgement_budget = std::cmp::max(
        crate::daemon::macos::pasteboard::AX_CONFIRM_DEADLINE,
        crate::daemon::macos::pasteboard::UNVERIFIED_GRACE,
    );
    let transaction_wait_budget = crate::daemon::macos::PASTE_MODIFIER_RELEASE_TIMEOUT
        + crate::daemon::desktop::inject::MACOS_CLIPBOARD_SETTLE_DELAY
        + acknowledgement_budget;
    let queued_then_own_transaction_budget = transaction_wait_budget + transaction_wait_budget;

    assert!(
        IPC_INSERT_RESPONSE_TIMEOUT > queued_then_own_transaction_budget,
        "IPC insertion response timeout must cover one queued macOS paste transaction \
         plus the command's own transaction"
    );
}

#[test]
fn shared_state_reports_phase_and_newest_transcript_byte_length() {
    let state = SharedState::new();
    state.set_phase("recording");
    state.remember_transcript("hi".to_string());
    state.remember_transcript("hello world".to_string());

    assert!(matches!(
        state.status(),
        IpcResponse::Status {
            phase,
            last_transcript_len: Some(11),
            detail: None,
        } if phase == "recording"
    ));
}

#[test]
fn last_transcript_summary_distinguishes_disabled_history() {
    let disabled = SharedState::with_history_limit(0);
    disabled.set_info(sample_daemon_info());
    let IpcResponse::Status {
        detail: Some(disabled_detail),
        ..
    } = disabled.status()
    else {
        panic!("expected populated status detail");
    };
    assert_eq!(
        last_transcript_summary(None, Some(&disabled_detail)),
        "history disabled"
    );

    let enabled = SharedState::new();
    enabled.set_info(sample_daemon_info());
    let IpcResponse::Status {
        detail: Some(enabled_detail),
        ..
    } = enabled.status()
    else {
        panic!("expected populated status detail");
    };
    assert_eq!(last_transcript_summary(None, Some(&enabled_detail)), "none");
    assert_eq!(
        last_transcript_summary(Some(12), Some(&enabled_detail)),
        "12 bytes"
    );
}

#[test]
fn history_snapshot_matrix() {
    struct Case {
        name: &'static str,
        history_limit: usize,
        remembered: &'static [&'static str],
        snapshot_limit: Option<usize>,
        expect: &'static [(usize, &'static str)],
    }

    let cases = [
        Case {
            name: "evicts oldest beyond history limit",
            history_limit: 2,
            remembered: &["first", "second", "third"],
            snapshot_limit: None,
            expect: &[(1, "third"), (2, "second")],
        },
        Case {
            name: "history limit zero remembers nothing",
            history_limit: 0,
            remembered: &["should not be kept"],
            snapshot_limit: None,
            expect: &[],
        },
        Case {
            name: "orders newest first with one-based index",
            history_limit: DEFAULT_TRANSCRIPT_HISTORY,
            remembered: &["oldest", "middle", "newest"],
            snapshot_limit: None,
            expect: &[(1, "newest"), (2, "middle"), (3, "oldest")],
        },
        Case {
            name: "respects snapshot limit",
            history_limit: DEFAULT_TRANSCRIPT_HISTORY,
            remembered: &["one", "two", "three"],
            snapshot_limit: Some(2),
            expect: &[(1, "three"), (2, "two")],
        },
    ];

    let failures: Vec<String> = cases
        .iter()
        .filter_map(|case| {
            let state = SharedState::with_history_limit(case.history_limit);
            for text in case.remembered {
                state.remember_transcript((*text).to_string());
            }
            let entries = state.history_snapshot(case.snapshot_limit);
            let actual: Vec<(usize, &str)> = entries
                .iter()
                .map(|entry| (entry.index, entry.preview.as_str()))
                .collect();
            (actual.as_slice() != case.expect).then(|| {
                format!(
                    "{}: expected {:?}, got {:?}",
                    case.name, case.expect, actual
                )
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

#[cfg(any(unix, target_os = "windows"))]
#[test]
fn history_ref_label_reproduces_last_transcript_wording_for_index_zero() {
    // Index 0 must keep the pre-history wording byte-identical, since
    // `copy-last` with no `N` argument is the common case and existing
    // docs/scripts quote this exact phrase.
    assert_eq!(history_ref_label(0), "last transcript");
    assert_eq!(history_ref_label(1), "transcript 2");
    assert_eq!(history_ref_label(4), "transcript 5");
}

#[cfg(any(unix, target_os = "windows"))]
#[test]
fn resolve_transcript_matrix() {
    struct Case {
        name: &'static str,
        /// `None` uses `SharedState::new()`'s default history limit.
        history_limit: Option<usize>,
        remembered: &'static [&'static str],
        index: usize,
        expect: Result<&'static str, &'static str>,
    }

    let cases = [
        Case {
            name: "history disabled reported first",
            history_limit: Some(0),
            remembered: &[],
            index: 0,
            expect: Err("transcript history is disabled (daemon.transcript_history = 0)"),
        },
        Case {
            name: "empty history when enabled but unused",
            history_limit: None,
            remembered: &[],
            index: 0,
            expect: Err("no transcript has been captured in this daemon session"),
        },
        Case {
            name: "out of range index singular",
            history_limit: None,
            remembered: &["only one"],
            index: 1,
            expect: Err("only 1 transcript remembered in this daemon session"),
        },
        Case {
            name: "out of range index plural",
            history_limit: None,
            remembered: &["only one", "second"],
            index: 5,
            expect: Err("only 2 transcripts remembered in this daemon session"),
        },
        Case {
            name: "in range index zero returns newest",
            history_limit: None,
            remembered: &["oldest", "newest"],
            index: 0,
            expect: Ok("newest"),
        },
        Case {
            name: "in range index one returns oldest",
            history_limit: None,
            remembered: &["oldest", "newest"],
            index: 1,
            expect: Ok("oldest"),
        },
    ];

    let failures: Vec<String> = cases
        .iter()
        .filter_map(|case| {
            let state = match case.history_limit {
                Some(limit) => SharedState::with_history_limit(limit),
                None => SharedState::new(),
            };
            for text in case.remembered {
                state.remember_transcript((*text).to_string());
            }
            let actual = state
                .resolve_transcript(case.index)
                .map_err(|err| err.to_string());
            let expect_owned: Result<String, String> =
                case.expect.map(str::to_string).map_err(str::to_string);
            (actual != expect_owned).then(|| {
                format!(
                    "{}: expected {:?}, got {:?}",
                    case.name, expect_owned, actual
                )
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

#[test]
fn preview_text_collapses_whitespace_and_trims() {
    let collapsed = preview_text("  line one\n  line two\t\tline three  ");
    assert_eq!(collapsed, "line one line two line three");
}

#[test]
fn preview_text_truncates_long_text_on_a_char_boundary() {
    // 'e' with combining characters would risk a byte-boundary panic if
    // truncation were byte-based instead of char-based; a plain
    // multi-byte codepoint repeated past the 72-char cap is enough to
    // catch that regression.
    let long = "é".repeat(80);

    let preview = preview_text(&long);

    assert_eq!(preview.chars().count(), 73); // 72 chars + '…'
    assert!(preview.ends_with('…'));
    assert!(preview.starts_with(&"é".repeat(72)));
}

fn sample_daemon_info() -> DaemonInfo {
    DaemonInfo {
        pid: 4242,
        model_name: "parakeet-tdt-0.6b-v3-Q8_0.gguf".to_string(),
        dtype: "Q8_0 (745 MB)".to_string(),
        mic_summary: "Test Mic, 48000 Hz mono input -> 16000 Hz mono model, F32".to_string(),
        backend: "CPU".to_string(),
        device: "cpu".to_string(),
        threads: 8,
        paste_mode: "standard",
        cleaning_summary: "on (12 rules)".to_string(),
        sounds_on: true,
        log_summary: Some("jsonl to /home/user/.parakit/logs".to_string()),
        hotkey_backend_label: Some("auto"),
    }
}

#[test]
fn shared_state_status_is_bare_until_info_is_set() {
    let state = SharedState::new();

    let IpcResponse::Status { detail, .. } = state.status() else {
        panic!("expected a Status response");
    };
    assert!(detail.is_none());
}

#[test]
fn shared_state_set_info_and_record_dictation_populate_status_detail() {
    let state = SharedState::new();
    state.set_info(sample_daemon_info());
    state.record_dictation();
    state.record_dictation();

    let IpcResponse::Status { detail, .. } = state.status() else {
        panic!("expected a Status response");
    };
    let detail = detail.expect("detail should be set after set_info");
    assert_eq!(detail.pid, 4242);
    assert_eq!(detail.dictation_count, 2);
    assert_eq!(detail.model, "parakeet-tdt-0.6b-v3-Q8_0.gguf");
    assert_eq!(detail.dtype, "Q8_0 (745 MB)");
    assert_eq!(detail.backend, "CPU");
    assert_eq!(detail.device, "cpu");
    assert_eq!(detail.threads, 8);
    assert_eq!(detail.paste_mode, "standard");
    assert!(detail.sounds);
    assert_eq!(detail.cleaning, "on (12 rules)");
    assert_eq!(
        detail.log.as_deref(),
        Some("jsonl to /home/user/.parakit/logs")
    );
    assert_eq!(detail.hotkey_backend.as_deref(), Some("auto"));
}

#[test]
fn status_detail_serde_round_trips() {
    let detail = StatusDetail {
        model_status: None,
        pid: 1234,
        uptime_secs: 5025,
        dictation_count: 7,
        mic: "Test Mic".to_string(),
        model: "model.gguf".to_string(),
        dtype: "Q8_0".to_string(),
        device: "cpu".to_string(),
        backend: "CPU".to_string(),
        threads: 4,
        paste_mode: "standard".to_string(),
        sounds: true,
        cleaning: "off".to_string(),
        log: None,
        hotkey_backend: None,
        history: "3 of 10".to_string(),
    };
    let response = IpcResponse::Status {
        phase: "idle".to_string(),
        last_transcript_len: Some(12),
        detail: Some(Box::new(detail.clone())),
    };

    let json = serde_json::to_string(&response).expect("status response should serialize");
    assert!(json.contains("\"model_status\":null"));
    let round_tripped: IpcResponse =
        serde_json::from_str(&json).expect("status response should deserialize");

    assert!(matches!(
        round_tripped,
        IpcResponse::Status {
            detail: Some(d),
            ..
        } if *d == detail
    ));
}

#[test]
fn model_status_keeps_phase_history_and_idle_deadline_independent() {
    use super::super::model_lifecycle::{ModelStatus, Residency};

    let state = SharedState::new();
    state.set_info(sample_daemon_info());
    state.set_phase("recording");
    state.remember_transcript("previous dictation".into());
    state.activity.ready();
    state.set_model_status(ModelStatus {
        residency: Residency::Offloaded,
        idle_minutes: 10,
        last_error: Some("model reload failed: missing local file".into()),
    });
    for _ in 0..3 {
        let response = state.status();
        let json = serde_json::to_string(&response).unwrap();
        let decoded: IpcResponse = serde_json::from_str(&json).unwrap();
        let IpcResponse::Status { phase, detail, .. } = decoded else {
            panic!("expected status");
        };
        assert_eq!(phase, "recording");
        assert_eq!(detail.unwrap().model_status, state.model_status());
        assert_eq!(state.resolve_transcript(0).unwrap(), "previous dictation");
    }
    assert_eq!(
        state.activity.remaining(Some(Duration::ZERO)),
        Some(Duration::ZERO)
    );
    state.with_insertion_lock(|| {
        assert_eq!(state.activity.remaining(Some(Duration::ZERO)), None);
    });
    assert_eq!(
        state.activity.remaining(Some(Duration::ZERO)),
        Some(Duration::ZERO)
    );
    assert_eq!(
        state.model_status().unwrap().residency,
        Residency::Offloaded
    );
}

#[test]
fn waiting_ipc_insertion_prevents_idle_expiration() {
    let state = Arc::new(SharedState::new());
    state.activity.ready();
    let _ = state.activity.changes().try_recv();
    let held = state.insertion.lock();
    let queued_state = Arc::clone(&state);
    let queued = thread::spawn(move || queued_state.with_insertion_lock(|| {}));
    state
        .activity
        .changes()
        .recv_timeout(Duration::from_secs(2))
        .unwrap();
    assert_eq!(state.activity.remaining(Some(Duration::ZERO)), None);
    drop(held);
    queued.join().unwrap();
    assert_eq!(
        state.activity.remaining(Some(Duration::ZERO)),
        Some(Duration::ZERO)
    );
}

#[test]
fn ipc_command_copy_last_serde_round_trips_with_index() {
    let command = IpcCommand::CopyLast { index: 1 };
    let json = serde_json::to_string(&command).expect("copy_last should serialize");
    assert_eq!(json, r#"{"copy_last":{"index":1}}"#);
    let round_tripped: IpcCommand =
        serde_json::from_str(&json).expect("copy_last should deserialize");
    assert!(matches!(round_tripped, IpcCommand::CopyLast { index: 1 }));
}

#[test]
fn ipc_command_history_serde_round_trips_with_and_without_limit() {
    let command: IpcCommand =
        serde_json::from_str(r#"{"history":{}}"#).expect("history with no limit should parse");
    assert!(matches!(command, IpcCommand::History { limit: None }));

    let command = IpcCommand::History { limit: Some(5) };
    let json = serde_json::to_string(&command).expect("history should serialize");
    let round_tripped: IpcCommand =
        serde_json::from_str(&json).expect("history should deserialize");
    assert!(matches!(
        round_tripped,
        IpcCommand::History { limit: Some(5) }
    ));
}

#[test]
fn ipc_response_history_serde_round_trips() {
    let response = IpcResponse::History {
        entries: vec![HistoryEntry {
            index: 1,
            age_secs: 12,
            chars: 47,
            preview: "the quick brown fox".to_string(),
        }],
    };

    let json = serde_json::to_string(&response).expect("history response should serialize");
    let round_tripped: IpcResponse =
        serde_json::from_str(&json).expect("history response should deserialize");

    assert!(matches!(
        round_tripped,
        IpcResponse::History { entries } if entries.len() == 1 && entries[0].preview == "the quick brown fox"
    ));
}

#[cfg(any(unix, target_os = "windows"))]
#[test]
fn parse_command_keeps_the_generic_context_for_unrelated_garbage() {
    let err = parse_command("not json").unwrap_err();
    assert_eq!(err.to_string(), "invalid control command");
}

#[cfg(any(unix, target_os = "windows"))]
#[test]
fn parse_command_still_accepts_the_current_wire_format() {
    let command = parse_command(r#"{"copy_last":{"index":2}}"#)
        .expect("current copy_last encoding should still parse");
    assert!(matches!(command, IpcCommand::CopyLast { index: 2 }));
}
