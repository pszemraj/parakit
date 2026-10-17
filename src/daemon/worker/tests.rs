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

fn test_worker_ctx(
    engine: FakeEngine,
    state: &Arc<SharedState>,
    sounds: Sounds,
    rx: Receiver<WorkerEvent>,
    model_idle_minutes: u64,
) -> WorkerCtx<FakeEngine> {
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
fn failed_transcription_keeps_previous_history_and_reports_failure() {
    let state = Arc::new(SharedState::with_history_limit(2));
    state.activity.ready();
    state.remember_transcript("previous dictation".to_owned());
    let (tx, rx) = crossbeam_channel::bounded(WORKER_QUEUE_CAPACITY);
    let (sounds, cues) = Sounds::test_channel();
    let ctx = test_worker_ctx(
        FakeEngine {
            transcriptions: Arc::new(AtomicUsize::new(0)),
            drops: Arc::new(AtomicUsize::new(0)),
            transcript: Err("incomplete native decoding"),
        },
        &state,
        sounds,
        rx,
        0,
    );
    let worker = std::thread::spawn(move || {
        worker_loop_with(ctx, None, || panic!("resident model must not reload"))
    });
    let (done_tx, done_rx) = crossbeam_channel::bounded(1);
    let capture = stopped_capture(Instant::now(), vec![0.1; 160], None, done_tx);
    tx.send(capture).unwrap();
    assert!(done_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap()
        .unwrap_err()
        .contains("incomplete native decoding"));
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
