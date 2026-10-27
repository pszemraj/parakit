//! Worker lifecycle and failure regressions.

use super::super::model_lifecycle::Residency;
use super::super::sounds::Cue;
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct FakeEngine {
    transcriptions: Arc<AtomicUsize>,
    drops: Arc<AtomicUsize>,
    transcript: std::result::Result<&'static str, &'static str>,
}

/// Exercise native delivery through the production worker without microphone input.
#[cfg(all(target_os = "linux", feature = "ibus"))]
#[test]
#[ignore = "requires an isolated IBus engine and focused native test application"]
fn live_native_worker_inserts_without_x11() {
    use crate::daemon::desktop::ibus::IbusClient;
    let client = IbusClient::start().unwrap();
    client.set_ready(true);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if client.snapshot().is_some_and(|target| target.eligible()) {
            break;
        }
        assert!(Instant::now() < deadline, "no eligible native test field");
        std::thread::sleep(Duration::from_millis(10));
    }
    let focus = crate::daemon::inject::FocusSource {
        backend: crate::daemon::desktop::linux_insertion::InsertionBackend::Auto,
        mode: PasteMode::Standard,
        client: Some(client.clone()),
        hotkey_route: crate::daemon::hotkey::LinuxHotkeyRoute::Ibus,
    }
    .capture()
    .unwrap();
    assert!(focus.is_native());
    run_live_worker(focus, "Worker café 世界. e\u{301} 👋", true);
    client.set_ready(false);
}

/// Exercise the existing clipboard paste path in an isolated focused X11 field.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires an isolated X11 server and focused text field"]
fn live_x11_worker_pastes_into_focused_field() {
    let focus = crate::daemon::inject::FocusSource {
        backend: crate::daemon::desktop::linux_insertion::InsertionBackend::Auto,
        mode: PasteMode::Standard,
        client: None,
        hotkey_route: crate::daemon::hotkey::LinuxHotkeyRoute::RegisteredX11,
    }
    .capture()
    .unwrap();
    assert!(!focus.is_native());
    run_live_worker(focus, "X11 worker café 世界", false);
}

/// Native rejection and terminal policy retain clipboard recovery with history disabled.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires an isolated X11 clipboard; never sends a paste chord"]
fn live_native_clipboard_recovery_without_history() {
    for text in [
        "native recovery café 世界",
        "native\ttab",
        "native\nmultiline",
    ] {
        let state = Arc::new(SharedState::with_history_limit(0));
        *state.focus_source.write() = crate::daemon::inject::FocusSource {
            backend: crate::daemon::desktop::linux_insertion::InsertionBackend::Ibus,
            mode: PasteMode::Terminal,
            client: None,
            hotkey_route: crate::daemon::hotkey::LinuxHotkeyRoute::Ibus,
        };
        let (tx, rx) = crossbeam_channel::bounded(2);
        let mut ctx = test_worker_ctx(
            FakeEngine {
                transcriptions: Arc::new(AtomicUsize::new(0)),
                drops: Arc::new(AtomicUsize::new(0)),
                transcript: Ok(text),
            },
            &state,
            Sounds::new(false),
            rx,
            0,
        );
        ctx.insert_transcripts = true;
        ctx.settings.paste_mode = PasteMode::Terminal;
        ctx.settings.keep_transcript_clipboard = false;
        let worker = std::thread::spawn(move || {
            worker_loop_with(ctx, None, || anyhow::bail!("unexpected model reload"));
        });
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let now = Instant::now();
        tx.send(WorkerEvent::Stopped {
            started_at: now,
            stopped_at: now,
            pcm: vec![0.1; 160],
            focus_at_start: Some(Box::new(FocusSnapshot::from_native(None, None))),
            activity: None,
            completion: Some(done_tx),
        })
        .unwrap();
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert_eq!(arboard::Clipboard::new().unwrap().get_text().unwrap(), text);
        assert!(state.resolve_transcript(0).is_err());
        drop(tx);
        worker.join().unwrap();
    }
}

/// Send a controlled transcript through production insertion and verify recovery text.
#[cfg(target_os = "linux")]
fn run_live_worker(focus: FocusSnapshot, text: &'static str, keep_clipboard: bool) {
    let state = Arc::new(SharedState::new());
    if focus.is_native() {
        *state.focus_source.write() = crate::daemon::inject::FocusSource {
            backend: crate::daemon::desktop::linux_insertion::InsertionBackend::Ibus,
            mode: PasteMode::Standard,
            client: None,
            hotkey_route: crate::daemon::hotkey::LinuxHotkeyRoute::Ibus,
        };
    }
    state.activity.ready();
    let (tx, rx) = crossbeam_channel::bounded(2);
    let mut ctx = test_worker_ctx(
        FakeEngine {
            transcriptions: Arc::new(AtomicUsize::new(0)),
            drops: Arc::new(AtomicUsize::new(0)),
            transcript: Ok(text),
        },
        &state,
        Sounds::new(false),
        rx,
        0,
    );
    ctx.insert_transcripts = true;
    ctx.settings.keep_transcript_clipboard = keep_clipboard;
    let worker = std::thread::spawn(move || {
        worker_loop_with(ctx, None, || anyhow::bail!("unexpected model reload"));
    });
    let (done_tx, done_rx) = crossbeam_channel::bounded(1);
    let now = Instant::now();
    tx.send(WorkerEvent::Stopped {
        started_at: now,
        stopped_at: now + Duration::from_millis(500),
        pcm: vec![0.1; TARGET_RATE as usize / 2],
        focus_at_start: Some(Box::new(focus)),
        activity: Some(state.activity.begin()),
        completion: Some(done_tx),
    })
    .unwrap();
    drop(tx);
    done_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    worker.join().unwrap();
    assert_eq!(state.resolve_transcript(0).unwrap(), text);
}

impl WorkerEngine for FakeEngine {
    fn transcribe(&self, _pcm: &[f32]) -> Result<String> {
        self.transcriptions.fetch_add(1, Ordering::SeqCst);
        self.transcript
            .map(str::to_owned)
            .map_err(anyhow::Error::msg)
    }
}

impl Drop for FakeEngine {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}

fn test_worker_ctx<E>(
    engine: E,
    state: &Arc<SharedState>,
    sounds: Sounds,
    rx: Receiver<WorkerEvent>,
    model_idle_minutes: u64,
) -> WorkerCtx<E> {
    let log = Arc::new(Logger::new(super::super::logging::LogLevel::Quiet));
    WorkerCtx {
        engine,
        recipe: EngineRecipe {
            model_path: std::path::PathBuf::new(),
            model_preset: None,
            threads: 1,
            qwen: parakit::qwen::QwenOptions::default(),
            device_mode: parakit::inference::DeviceMode::Cpu,
            #[cfg(feature = "bundled")]
            backend: String::new(),
            verbose: false,
        },
        settings: WorkerSettings {
            model_idle_minutes,
            data_log: None,
            paste_mode: PasteMode::Standard,
            keep_transcript_clipboard: false,
        },
        cleaner: None,
        sounds,
        notifier: Notifier::silent(Arc::clone(&log)),
        log,
        state: Arc::clone(state),
        insert_transcripts: false,
        rx,
        lifetime: state.shutdown.register(),
    }
}

#[test]
fn logging_metadata_stays_with_the_resident_session_until_reload() {
    struct MetadataEngine(Arc<AtomicUsize>);
    impl WorkerEngine for MetadataEngine {
        fn transcribe(&self, _: &[f32]) -> Result<String> {
            Ok("dictation".to_owned())
        }

        fn inference_metadata(&self, recipe: &EngineRecipe) -> Option<InferenceLogFields> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Some(InferenceLogFields {
                model: "fixture.gguf".to_owned(),
                model_preset: None,
                backend: "test".to_owned(),
                quantization: std::fs::read_to_string(&recipe.model_path).unwrap(),
                requested_device: recipe.device_mode,
                session_device: recipe.device_mode,
                confirmed_device: Some("cpu".to_owned()),
                qwen_language: None,
                qwen_max_new_tokens: None,
                native_api_version: None,
            })
        }
    }

    let root = crate::test_support::fixture_root("worker-metadata", "reload");
    let model_path = root.join("model-label");
    std::fs::write(&model_path, "Q8_0").unwrap();
    let state = Arc::new(SharedState::new());
    state.activity.ready();
    let lease = state.activity.begin();
    let calls = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = crossbeam_channel::bounded(2);
    let mut ctx = test_worker_ctx(
        MetadataEngine(Arc::clone(&calls)),
        &state,
        Sounds::new(false),
        rx,
        1,
    );
    ctx.recipe.model_path = model_path.clone();
    let log_dir = root.join("logs");
    ctx.settings.data_log = Some(Arc::new(DataLogger::new(log_dir.clone())));
    let reload_calls = Arc::clone(&calls);
    let worker = std::thread::spawn(move || {
        worker_loop_with(ctx, Some(Duration::from_millis(20)), || {
            Ok(MetadataEngine(Arc::clone(&reload_calls)))
        });
    });
    let transcribe = || {
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        tx.send(stopped_capture(
            Instant::now(),
            vec![0.1; 160],
            Some(state.activity.begin()),
            done_tx,
        ))
        .unwrap();
        done_rx
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
    };
    transcribe();
    std::fs::write(&model_path, "F16").unwrap();
    transcribe();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    drop(lease);
    wait_for_state(&state, Residency::Offloaded);
    transcribe();
    drop(tx);
    worker.join().unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    let rows: Vec<serde_json::Value> = std::fs::read_dir(log_dir)
        .unwrap()
        .flat_map(|entry| {
            std::fs::read_to_string(entry.unwrap().path())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect::<Vec<_>>()
        })
        .collect();
    let quantizations: Vec<_> = rows
        .iter()
        .filter_map(|row| row["inference"]["quantization"].as_str())
        .collect();
    assert_eq!(quantizations, ["Q8_0", "Q8_0", "F16"]);
}

fn next_cue(cues: &Receiver<Cue>) -> Cue {
    cues.recv_timeout(Duration::from_secs(1)).unwrap()
}

fn stopped_capture(
    now: Instant,
    pcm: Vec<f32>,
    activity: Option<ActivityGuard>,
    completion: Sender<Result<(), String>>,
) -> WorkerEvent {
    WorkerEvent::Stopped {
        started_at: now,
        stopped_at: now,
        pcm,
        focus_at_start: None,
        activity,
        completion: Some(completion),
    }
}

fn wait_for_state(state: &SharedState, residency: Residency) {
    let deadline = Instant::now() + Duration::from_secs(1);
    while state
        .model_status()
        .is_none_or(|status| status.residency != residency)
    {
        assert!(
            Instant::now() < deadline,
            "worker did not reach {residency:?}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn silence_gate_skips_empty_and_quiet_audio_only() {
    assert!(capture_should_skip(&[]));
    assert!(capture_should_skip(&[0.0; 160]));
    assert!(capture_should_skip(&[0.0001; 160]));
    assert!(!capture_should_skip(&[0.0, 0.2]));
    assert!(!capture_should_skip(&[0.01; 16]));
    assert!(!capture_should_skip(&[SILENCE_PEAK_THRESHOLD]));
    assert!(!capture_should_skip(&[-SILENCE_PEAK_THRESHOLD]));
    assert!(capture_should_skip(&[SILENCE_RMS_THRESHOLD * 0.5; 160]));
    assert!(!capture_should_skip(&[SILENCE_RMS_THRESHOLD; 160]));
}

#[test]
fn worker_plays_error_cue_when_no_speech_is_detected() {
    // Silence stops at the gate; audible noise reaches inference and comes
    // back blank. Neither inserts text, so neither may sound successful.
    for (pcm, inferred) in [(vec![0.0; 160], 0), (vec![0.01; 16], 1)] {
        let state = Arc::new(SharedState::with_history_limit(2));
        state.activity.ready();
        let (tx, rx) = crossbeam_channel::bounded(WORKER_QUEUE_CAPACITY);
        let (sounds, cues) = Sounds::test_channel();
        let transcriptions = Arc::new(AtomicUsize::new(0));
        let ctx = test_worker_ctx(
            FakeEngine {
                transcriptions: Arc::clone(&transcriptions),
                drops: Arc::new(AtomicUsize::new(0)),
                transcript: Ok(" "),
            },
            &state,
            sounds,
            rx,
            0,
        );
        let worker = std::thread::spawn(move || {
            worker_loop_with(ctx, None, || -> Result<FakeEngine> {
                anyhow::bail!("resident model must not reload")
            });
        });

        let now = Instant::now();
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        tx.send(stopped_capture(now, pcm, None, done_tx)).unwrap();
        assert_eq!(
            done_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Err("dictation produced no transcript".to_string())
        );
        assert!(matches!(next_cue(&cues), Cue::Error));
        drop(tx);
        worker.join().unwrap();
        assert!(cues.try_recv().is_err());
        assert_eq!(transcriptions.load(Ordering::SeqCst), inferred);
        assert!(state.resolve_transcript(0).is_err());
    }
}

#[test]
fn worker_loop_offloads_then_reloads_and_transcribes_queued_capture() {
    exercise_blocked_reload(ReloadCase::Released);
}

#[test]
fn failed_or_skipped_transcription_keeps_previous_history_without_recovery_hint() {
    const CHILD: &str = "PARAKIT_TEST_RETAINED_HISTORY";
    let skipped = match std::env::var(CHILD).as_deref() {
        Ok("skipped") => true,
        Ok("failed") => false,
        _ => {
            for case in ["failed", "skipped"] {
                let output = crate::test_support::exact_test_command(
                    "daemon::worker::tests::failed_or_skipped_transcription_keeps_previous_history_without_recovery_hint",
                )
                .env(CHILD, case)
                .output()
                .unwrap();
                assert!(output.status.success(), "{case}: {output:?}");
                let stderr = String::from_utf8_lossy(&output.stderr);
                assert!(!stderr.contains("transcript saved"), "{case}: {stderr}");
                assert!(!stderr.contains("copy-last"), "{case}: {stderr}");
            }
            return;
        }
    };
    let state = Arc::new(SharedState::with_history_limit(2));
    state.activity.ready();
    state.remember_transcript("previous dictation".to_owned());
    let (tx, rx) = crossbeam_channel::bounded(WORKER_QUEUE_CAPACITY);
    let (sounds, cues) = Sounds::test_channel();
    let mut ctx = test_worker_ctx(
        FakeEngine {
            transcriptions: Arc::new(AtomicUsize::new(0)),
            drops: Arc::new(AtomicUsize::new(0)),
            transcript: if skipped {
                Ok("\x1b")
            } else {
                Err("incomplete native decoding")
            },
        },
        &state,
        sounds,
        rx,
        0,
    );
    // The control-only transcript is skipped before any clipboard write or input.
    ctx.insert_transcripts = skipped;
    ctx.log = Arc::new(Logger::new(super::super::logging::LogLevel::Normal));
    let worker = std::thread::spawn(move || {
        worker_loop_with(ctx, None, || panic!("resident model must not reload"))
    });
    let (done_tx, done_rx) = crossbeam_channel::bounded(1);
    let capture = stopped_capture(Instant::now(), vec![0.1; 160], None, done_tx);
    tx.send(capture).unwrap();
    let completed = done_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    if skipped {
        completed.unwrap();
    } else {
        assert!(completed
            .unwrap_err()
            .contains("incomplete native decoding"));
    }
    assert!(matches!(next_cue(&cues), Cue::Error));
    drop(tx);
    worker.join().unwrap();
    assert_eq!(state.resolve_transcript(0).unwrap(), "previous dictation");
    assert!(state.resolve_transcript(1).is_err());
}

#[test]
fn worker_announces_reload_then_readiness_while_ptt_held() {
    exercise_blocked_reload(ReloadCase::Held);
}

#[test]
fn worker_failed_reload_never_announces_readiness() {
    exercise_blocked_reload(ReloadCase::Failed);
}

#[test]
fn worker_retries_transient_reload_failure_after_recording_stops() {
    exercise_blocked_reload(ReloadCase::Recovered);
}

#[test]
fn worker_reports_immediate_retry_after_an_early_release() {
    exercise_blocked_reload(ReloadCase::ReleasedRecovered);
}

#[test]
fn worker_shutdown_during_reload_drops_session_and_queued_capture() {
    exercise_blocked_reload(ReloadCase::Shutdown);
}

#[derive(Clone, Copy, PartialEq)]
enum ReloadCase {
    Held,
    Released,
    Failed,
    Recovered,
    ReleasedRecovered,
    Shutdown,
}

fn exercise_blocked_reload(case: ReloadCase) {
    let state = Arc::new(SharedState::with_history_limit(2));
    state.activity.ready();
    let (tx, rx) = crossbeam_channel::bounded(WORKER_QUEUE_CAPACITY);
    let reloads = Arc::new(AtomicUsize::new(0));
    let transcriptions = Arc::new(AtomicUsize::new(0));
    let drops = Arc::new(AtomicUsize::new(0));
    let loader_activity = Arc::clone(&state.activity);
    let loader_reloads = Arc::clone(&reloads);
    let loader_transcriptions = Arc::clone(&transcriptions);
    let loader_drops = Arc::clone(&drops);
    let (load_started_tx, load_started_rx) = crossbeam_channel::bounded(1);
    let (load_release_tx, load_release_rx) = crossbeam_channel::bounded(1);
    let (sounds, cues) = Sounds::test_channel();
    let ctx = test_worker_ctx(
        FakeEngine {
            transcriptions: Arc::clone(&transcriptions),
            drops: Arc::clone(&drops),
            transcript: Ok("reloaded dictation"),
        },
        &state,
        sounds,
        rx,
        1,
    );

    let worker = std::thread::spawn(move || {
        let load = move || {
            // A reload may admit other activity; it must not inherit the
            // gate mutex held by the offload decision.
            let _admission = loader_activity.begin();
            let attempt = loader_reloads.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 {
                load_started_tx.send(()).unwrap();
                load_release_rx.recv().unwrap();
            }
            if case == ReloadCase::Failed
                || (matches!(case, ReloadCase::Recovered | ReloadCase::ReleasedRecovered)
                    && attempt == 0)
            {
                anyhow::bail!("reload failed in test");
            }
            Ok(FakeEngine {
                transcriptions: Arc::clone(&loader_transcriptions),
                drops: Arc::clone(&loader_drops),
                transcript: Ok("reloaded dictation"),
            })
        };
        worker_loop_with(ctx, Some(Duration::from_millis(10)), load);
    });

    wait_for_state(&state, Residency::Offloaded);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    assert_eq!(reloads.load(Ordering::SeqCst), 0);
    assert!(cues.try_recv().is_err(), "offloading alone is silent");

    // Keep the reloaded model resident until all post-completion checks
    // have observed it; the capture lease moves into `Stopped` below.
    let verify_residency = state.activity.begin();
    let recording = state.activity.begin();
    let (completion_tx, completion_rx) = crossbeam_channel::bounded(1);
    let now = Instant::now();
    let capture_active = Arc::new(AtomicBool::new(true));
    tx.send(WorkerEvent::Started {
        recording: Arc::clone(&capture_active),
    })
    .unwrap();
    load_started_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    assert!(matches!(next_cue(&cues), Cue::Reload));
    assert!(
        cues.try_recv().is_err(),
        "must not announce readiness during reload"
    );
    let stopped = stopped_capture(now, vec![0.01; 16], Some(recording), completion_tx);
    // The released capture remains queued; a held capture stops only after
    // readiness is heard below.
    let held_capture = if matches!(
        case,
        ReloadCase::Held | ReloadCase::Failed | ReloadCase::Recovered
    ) {
        Some(stopped)
    } else {
        capture_active.store(false, Ordering::Release);
        tx.send(stopped).unwrap();
        None
    };
    let completion_before_reload = completion_rx.try_recv();
    if case == ReloadCase::Shutdown {
        assert_eq!(state.model_status().unwrap().residency, Residency::Loading);
        assert!(!state
            .shutdown
            .request_and_wait(&state.activity, Duration::ZERO));
    }
    load_release_tx.send(()).unwrap();
    if let Some(stopped) = held_capture {
        if case == ReloadCase::Held {
            assert!(matches!(next_cue(&cues), Cue::Start(_)));
        } else {
            wait_for_state(&state, Residency::Offloaded);
            assert_eq!(reloads.load(Ordering::SeqCst), 1);
            assert!(matches!(next_cue(&cues), Cue::Error));
            assert!(
                cues.try_recv().is_err(),
                "failure must not announce readiness"
            );
        }
        capture_active.store(false, Ordering::Release);
        tx.send(stopped).unwrap();
    } else if case == ReloadCase::ReleasedRecovered {
        assert!(matches!(next_cue(&cues), Cue::Error));
    }
    assert_eq!(
        completion_before_reload,
        Err(crossbeam_channel::TryRecvError::Empty)
    );

    if case == ReloadCase::Shutdown {
        assert!(state
            .shutdown
            .request_and_wait(&state.activity, Duration::from_secs(1)));
        // Shutdown must not report completion with a live native session.
        assert_eq!(drops.load(Ordering::SeqCst), 2);
        worker.join().unwrap();
        assert!(
            cues.try_recv().is_err(),
            "shutdown must not announce readiness"
        );
        // The producer owns the channel's remaining queued payloads until
        // it also stops; no receiver may process them after worker exit.
        drop(tx);
        assert_eq!(reloads.load(Ordering::SeqCst), 1);
        assert_eq!(transcriptions.load(Ordering::SeqCst), 0);
        assert!(state.resolve_transcript(0).is_err());
        assert_eq!(
            completion_rx.try_recv(),
            Err(crossbeam_channel::TryRecvError::Disconnected)
        );
        drop(verify_residency);
        assert_eq!(
            state.activity.remaining(Some(Duration::ZERO)),
            Some(Duration::ZERO)
        );
        return;
    }

    let completion = completion_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    assert_eq!(
        reloads.load(Ordering::SeqCst),
        if matches!(
            case,
            ReloadCase::Failed | ReloadCase::Recovered | ReloadCase::ReleasedRecovered
        ) {
            2
        } else {
            1
        }
    );
    if case == ReloadCase::Failed {
        assert!(completion.unwrap_err().contains("reload failed in test"));
        assert_eq!(transcriptions.load(Ordering::SeqCst), 0);
        assert_eq!(
            state.model_status().unwrap().residency,
            Residency::Offloaded
        );
        assert!(state.resolve_transcript(0).is_err());
        assert!(matches!(next_cue(&cues), Cue::Error));
    } else {
        assert_eq!(completion, Ok(()));
        assert_eq!(transcriptions.load(Ordering::SeqCst), 1);
        assert_eq!(state.model_status().unwrap().residency, Residency::Loaded);
        assert_eq!(state.resolve_transcript(0).unwrap(), "reloaded dictation");
        assert!(matches!(next_cue(&cues), Cue::Success));
    }
    assert!(state.resolve_transcript(1).is_err());

    if case == ReloadCase::Held {
        // A later PTT with the model resident needs only the normal cue.
        let next = Arc::new(AtomicBool::new(true));
        tx.send(WorkerEvent::Started {
            recording: Arc::clone(&next),
        })
        .unwrap();
        assert!(matches!(next_cue(&cues), Cue::Start(_)));
        next.store(false, Ordering::Release);
        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        tx.send(stopped_capture(
            now,
            vec![0.01; 16],
            Some(state.activity.begin()),
            done_tx,
        ))
        .unwrap();
        assert_eq!(
            done_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Ok(())
        );
        assert!(matches!(next_cue(&cues), Cue::Success));
        assert_eq!(reloads.load(Ordering::SeqCst), 1);
        assert_eq!(transcriptions.load(Ordering::SeqCst), 2);
    }

    drop(verify_residency);
    drop(tx);
    worker.join().unwrap();
    assert_eq!(
        completion_rx.try_recv(),
        Err(crossbeam_channel::TryRecvError::Disconnected)
    );
    assert!(cues.try_recv().is_err());
    assert_eq!(
        drops.load(Ordering::SeqCst),
        if case == ReloadCase::Failed { 1 } else { 2 }
    );
}
