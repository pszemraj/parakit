//! Opt-in acoustic capture through the production coordinator and ASR worker.
//!
//! Capture, history, and residency checks always gate acceptance. Acoustic ASR
//! varies with room sound and cues; exact first/last-word parity is reported
//! separately. Set `PARAKIT_LIVE_REQUIRE_MATCHING_EDGES` to make that speech
//! quality comparison an additional acceptance gate. All transcripts and
//! mismatches remain in the report regardless of the selected gate.

use super::{repository_path, AudioCapture, Duration, Instant, LogLevel, Logger, Notifier};
use crate::daemon::{
    engine_runtime::EngineRecipe,
    inject::{FocusSource, PasteMode},
    ipc::SharedState,
    model_lifecycle::Residency,
    recording::{spawn_recording_coordinator, DictationMode, HotkeyTransition},
    sounds::Sounds,
    worker::{spawn_worker, WorkerCtx, WorkerSettings, WORKER_QUEUE_CAPACITY},
};
use anyhow::{ensure, Context, Result};
use crossbeam_channel::{bounded, unbounded, Sender};
use parakit::{data_log::DataLogger, inference::DeviceMode, qwen::QwenOptions};
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use serde_json::{json, Value};
use std::{fs::File, io::BufReader, path::Path, sync::Arc, thread};

fn wait_until(timeout: Duration, mut ready: impl FnMut() -> bool) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if ready() {
            return Ok(());
        }
        ensure!(
            Instant::now() < deadline,
            "native worker condition timed out"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn edge_words(text: &str) -> Value {
    let words: Vec<_> = text
        .split_whitespace()
        .map(|word| {
            word.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .filter(|word| !word.is_empty())
        .collect();
    json!({
        "first": &words[..words.len().min(3)],
        "last": &words[words.len().saturating_sub(3)..],
    })
}

fn capture_trial(
    tx: &Sender<HotkeyTransition>,
    state: &SharedState,
    audio: &AudioCapture,
    output: &OutputStreamHandle,
    wav: &Path,
    count: usize,
    expected_residency: Residency,
) -> Result<Value> {
    // Decode and prepare output before the press. Keep disk reads, decoder
    // startup, and output-device creation out of the speech-onset interval.
    let decoder = Decoder::new(BufReader::new(File::open(wav)?))?;
    let playback_duration = decoder
        .total_duration()
        .context("WAV duration is unknown")?;
    let source = rodio::buffer::SamplesBuffer::new(
        decoder.channels(),
        decoder.sample_rate(),
        decoder.convert_samples::<f32>().collect::<Vec<_>>(),
    );
    let playback = Sink::try_new(output)?;
    let previous = state.resolve_transcript(0).ok();
    let initial = state.model_status();
    ensure!(
        initial.as_ref().is_some_and(|status| {
            status.residency == expected_residency && status.last_error.is_none()
        }),
        "expected {expected_residency:?} model before capture, got {initial:?}"
    );
    let started = Instant::now();
    tx.send(HotkeyTransition::Pressed {
        at: started,
        mode: DictationMode::Plain,
    })?;
    // The shared output stream is already open; enqueue immediately after the
    // logical press while the coordinator performs real focus/audio admission.
    playback.append(source);
    let playback_enqueued = started.elapsed();
    let playback_deadline = Instant::now() + playback_duration + Duration::from_secs(5);
    let mut observations = Vec::new();
    let mut last = None;
    let mut saw_capture = false;
    let mut stopped_at = None;
    let mut loaded_at = None;
    let mut observe = || {
        let capturing = audio.handle.test_is_recording();
        let residency = state.model_status().map(|status| status.residency);
        let elapsed = started.elapsed().as_secs_f64();
        saw_capture |= capturing;
        if saw_capture && !capturing && stopped_at.is_none() {
            stopped_at = Some(elapsed);
        }
        if residency == Some(Residency::Loaded) && loaded_at.is_none() {
            loaded_at = Some(elapsed);
        }
        if last != Some((capturing, residency)) {
            let observation = json!({
                "seconds_after_press": elapsed,
                "capture_active": capturing,
                "model_residency": residency,
            });
            println!("parakit live-worker observation: {observation}");
            observations.push(observation);
            last = Some((capturing, residency));
        }
    };
    loop {
        observe();
        if playback.empty() {
            break;
        }
        ensure!(Instant::now() < playback_deadline, "WAV playback timed out");
        thread::sleep(Duration::from_millis(10));
    }
    let playback_finished = started.elapsed();
    // Preserve the acoustic tail after the source finishes on the shared stream.
    let tail_until = Instant::now() + Duration::from_millis(250);
    while Instant::now() < tail_until {
        observe();
        thread::sleep(Duration::from_millis(10));
    }
    let released = Instant::now();
    let residency_at_release = state.model_status();
    tx.send(HotkeyTransition::Released { at: released })?;
    wait_until(Duration::from_secs(120), || {
        observe();
        state.resolve_transcript(count - 1).is_ok()
            && state
                .activity
                .remaining(Some(Duration::from_secs(1)))
                .is_some()
    })?;
    ensure!(saw_capture, "no active microphone capture was observed");
    ensure!(
        !audio.handle.test_is_recording(),
        "microphone capture remained active after completion"
    );
    let completed_model = state.model_status();
    ensure!(
        completed_model.as_ref().is_some_and(|status| {
            status.residency == Residency::Loaded && status.last_error.is_none()
        }),
        "model was not loaded after capture: {completed_model:?}"
    );
    let transcript = state.resolve_transcript(0)?;
    ensure!(!transcript.trim().is_empty(), "empty native transcript");
    ensure!(
        state.resolve_transcript(count).is_err(),
        "duplicate history entry"
    );
    if let Some(previous) = previous {
        ensure!(
            state.resolve_transcript(1)? == previous,
            "older history changed"
        );
    }
    let early_release = residency_at_release
        .as_ref()
        .is_some_and(|status| status.residency == Residency::Loading)
        && stopped_at
            .zip(loaded_at)
            .is_some_and(|(stop, load)| stop < load);
    Ok(json!({
        "wav": wav,
        "playback_transport": "rodio_predecoded_shared_output_stream",
        "playback_source_seconds": playback_duration.as_secs_f64(),
        "playback_enqueue_seconds_after_press": playback_enqueued.as_secs_f64(),
        "playback_finished_seconds_after_press": playback_finished.as_secs_f64(),
        "hardware_playback_onset_measured": false,
        "logical_press": true,
        "physical_hotkey_tested": false,
        "initial_model": initial,
        "model_at_release": residency_at_release,
        "model_after_completion": completed_model,
        "release_seconds_after_press": released.duration_since(started).as_secs_f64(),
        "completion_seconds_after_press": started.elapsed().as_secs_f64(),
        "capture_started": saw_capture,
        "capture_stop_observed_seconds": stopped_at,
        "model_loaded_observed_seconds": loaded_at,
        "released_and_capture_stopped_before_reload_ready": early_release,
        "observations": observations,
        "history_count": count,
        "pipeline_checks_passed": true,
        "transcript": transcript,
        "edge_words": edge_words(&transcript),
    }))
}

#[test]
#[ignore = "requires macOS microphone/speakers and PARAKIT_LIVE_MODEL, DEVICE, WAV, REPORT; PARAKIT_LIVE_REQUIRE_MATCHING_EDGES optionally gates acoustic word parity"]
fn microphone_coordinator_worker_reloads_after_idle() {
    let model = repository_path("PARAKIT_LIVE_MODEL");
    let wav = repository_path("PARAKIT_LIVE_WAV");
    let report_path = repository_path("PARAKIT_LIVE_REPORT");
    let second = std::env::var_os("PARAKIT_LIVE_SECOND_WAV")
        .map(|_| repository_path("PARAKIT_LIVE_SECOND_WAV"));
    let device = match std::env::var("PARAKIT_LIVE_DEVICE").as_deref() {
        Ok("cpu") => DeviceMode::Cpu,
        Ok("gpu") => DeviceMode::Gpu,
        _ => panic!("PARAKIT_LIVE_DEVICE must be cpu or gpu"),
    };
    let logs = report_path.with_extension("logs");
    assert!(!logs.exists(), "use a fresh report/log destination");
    std::fs::create_dir(&logs).unwrap();
    let (_output_stream, output_handle) = OutputStream::try_default().unwrap();
    let log = Arc::new(Logger::new(LogLevel::Verbose));
    let recipe = EngineRecipe {
        model_path: model.clone(),
        model_preset: None,
        threads: 6,
        device_mode: device,
        #[cfg(feature = "bundled")]
        backend: String::new(),
        qwen: QwenOptions::default(),
        verbose: true,
    };
    let (engine, device_summary) = recipe.open(&log).unwrap();
    let recipe = EngineRecipe {
        #[cfg(feature = "bundled")]
        backend: engine.backend().to_owned(),
        ..recipe
    };
    let backend = engine.backend().to_owned();
    let notifier = Notifier::silent(Arc::clone(&log));
    let audio = AudioCapture::open(Arc::clone(&log), notifier.clone()).unwrap();
    let state = Arc::new(SharedState::with_history_limit(8));
    let (worker_tx, worker_rx) = bounded(WORKER_QUEUE_CAPACITY);
    let cues_enabled = std::env::var_os("PARAKIT_LIVE_CUES").is_some();
    let require_matching_edges = std::env::var_os("PARAKIT_LIVE_REQUIRE_MATCHING_EDGES").is_some();
    let worker = spawn_worker(WorkerCtx {
        engine,
        recipe,
        settings: WorkerSettings {
            context_on_default_hotkey: false,
            context_dictations: 3,
            context_max_age: Duration::from_secs(600),
            model_idle_minutes: 1,
            data_log: Some(Arc::new(DataLogger::new(logs.clone()))),
            paste_mode: PasteMode::Standard,
            keep_transcript_clipboard: false,
        },
        cleaner: None,
        sounds: Sounds::new(cues_enabled),
        log: Arc::clone(&log),
        notifier,
        state: Arc::clone(&state),
        insert_transcripts: false,
        rx: worker_rx,
        lifetime: state.shutdown.register(),
    });
    let (hotkey_tx, hotkey_rx) = unbounded();
    let coordinator = spawn_recording_coordinator(
        hotkey_rx,
        worker_tx.clone(),
        audio.handle.clone(),
        log,
        Arc::clone(&state.activity),
        FocusSource::default(),
    )
    .unwrap();
    state.activity.ready();
    let mut trials = Vec::new();
    let planned_trials = 2 * (1 + usize::from(second.is_some()));
    let result = (|| -> Result<()> {
        thread::sleep(Duration::from_secs(1));
        wait_until(Duration::from_secs(5), || state.model_status().is_some())?;
        for path in std::iter::once(&wav).chain(second.as_ref()) {
            let control = capture_trial(
                &hotkey_tx,
                &state,
                &audio,
                &output_handle,
                path,
                trials.len() + 1,
                Residency::Loaded,
            )?;
            trials.push(control.clone());
            let idle_at = Instant::now();
            wait_until(Duration::from_secs(75), || {
                state
                    .model_status()
                    .is_some_and(|status| status.residency == Residency::Offloaded)
            })?;
            let idle_elapsed = idle_at.elapsed();
            let mut reloaded = capture_trial(
                &hotkey_tx,
                &state,
                &audio,
                &output_handle,
                path,
                trials.len() + 1,
                Residency::Offloaded,
            )?;
            reloaded["idle_before_press_seconds"] = json!(idle_elapsed.as_secs_f64());
            let edges_match = reloaded["edge_words"] == control["edge_words"];
            reloaded["edges_match_loaded_control"] = json!(edges_match);
            trials.push(reloaded);
        }
        Ok(())
    })();
    // On playback or observation errors, closing the hotkey channel makes the
    // coordinator stop any active capture. Closing its worker senders then
    // drains/ends the real worker before the native session can be destroyed.
    drop(hotkey_tx);
    coordinator.join().unwrap();
    drop(worker_tx);
    worker.join().unwrap();
    let pipeline_checks_passed = result.is_ok()
        && trials.len() == planned_trials
        && trials
            .iter()
            .all(|trial| trial["pipeline_checks_passed"] == true);
    let speech_boundary_checks_passed = pipeline_checks_passed
        && trials
            .iter()
            .filter_map(|trial| trial["edges_match_loaded_control"].as_bool())
            .all(|matched| matched);
    let overall_passed =
        pipeline_checks_passed && (!require_matching_edges || speech_boundary_checks_passed);
    let acceptance = result.and_then(|()| {
        ensure!(
            pipeline_checks_passed,
            "native pipeline checks were incomplete"
        );
        ensure!(
            !require_matching_edges || speech_boundary_checks_passed,
            "recognized first/last words differ across idle reload"
        );
        Ok(())
    });
    let report = json!({
        "model": model,
        "backend": backend,
        "requested_device": device,
        "device_summary": device_summary,
        "microphone": audio.mic_info().map(|info| info.summary()),
        "cues_requested": cues_enabled,
        "subjective_cue_audibility_tested": false,
        "insertion_enabled": false,
        "playback_transport": "rodio_predecoded_shared_output_stream",
        "pipeline_checks_passed": pipeline_checks_passed,
        "speech_boundary_checks_passed": speech_boundary_checks_passed,
        "require_matching_edges": require_matching_edges,
        "overall_passed": overall_passed,
        "model_idle_minutes": 1,
        "transcription_logs": logs,
        "trials": trials,
        "error": acceptance.as_ref().err().map(|error| format!("{error:#}")),
    });
    std::fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    acceptance
        .context("native capture/coordinator/worker acceptance")
        .unwrap();
}
