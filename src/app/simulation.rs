//! WAV validation through the production worker, including idle reloads.

use super::*;
use crate::daemon::model_lifecycle::{idle_timeout, Residency};
use parakit::audio_file::prepare_wav_for_model;
use std::thread;

/// Run one or more acknowledged WAV dictations without microphone or insertion.
///
/// # Arguments
///
/// * `cli` - Global output flags.
/// * `start` - Runtime and simulation options.
/// * `config` - Validated daemon defaults.
/// * `settings` - Effective settings shared with normal daemon startup.
/// * `log` - Shared process logger.
/// * `audio_paths` - Real WAV fixtures to replay in order on one worker.
///
/// # Returns
///
/// Success after all requested transcripts and residency checks complete.
///
/// # Errors
///
/// Fails on preparation, loading, worker errors, changed transcripts, or an
/// unexpected residency after a requested idle interval.
pub(super) fn run_ptt_audio_simulation(
    cli: &Cli,
    start: &StartCli,
    config: &ConfigFile,
    settings: WorkerSettings,
    log: Arc<Logger>,
    audio_paths: &[PathBuf],
) -> Result<()> {
    let verbose = cli.effective_verbose(config);
    let cleaner = build_cli_cleaner(start, config)?.map(Arc::new);
    let timeout = idle_timeout(settings.model_idle_minutes);
    let idle = Duration::from_secs(start.simulate_ptt_idle_seconds.unwrap_or(0));
    let repeats = start.simulate_ptt_repeat.map_or(1, NonZeroUsize::get);
    // Validate before model initialization rather than overflowing a sleep later.
    if Instant::now().checked_add(idle).is_none() {
        anyhow::bail!("simulated idle interval exceeds the supported clock range");
    }
    let prepare_started = Instant::now();
    let wavs = audio_paths
        .iter()
        .map(|path| prepare_wav_for_model(path))
        .collect::<Result<Vec<_>>>()?;
    log.verbose(format!(
        "parakit: simulated audio prepared in {:.0}ms ({} files)",
        prepare_started.elapsed().as_secs_f32() * 1000.0,
        wavs.len()
    ));
    let OpenedEngine { engine, recipe, .. } =
        open_cli_engine(start, config, verbose, cli.quiet || !verbose, &log)?;
    let context_enabled = engine.qwen_options().is_some()
        && settings.context_on_default_hotkey
        && settings.context_dictations != 0
        && config.daemon.effective_transcript_history() != 0;
    let state = Arc::new(daemon::ipc::SharedState::with_history_limit(
        config.daemon.effective_transcript_history(),
    ));
    let worker_lifetime = state.shutdown.register();
    let (tx, rx) = bounded::<WorkerEvent>(WORKER_QUEUE_CAPACITY);
    let worker = spawn_worker(WorkerCtx {
        engine,
        recipe,
        settings,
        cleaner,
        sounds: Sounds::new(false),
        log: Arc::clone(&log),
        notifier: Notifier::silent(Arc::new(Logger::new(LogLevel::Quiet))),
        state: Arc::clone(&state),
        insert_transcripts: false,
        rx,
        lifetime: worker_lifetime,
    });
    state.activity.ready();
    let result = (|| {
        let mut baselines = vec![None; wavs.len()];
        for cycle in 1..=repeats {
            for (baseline, (audio_path, wav)) in
                baselines.iter_mut().zip(audio_paths.iter().zip(&wavs))
            {
                let audio_secs = wav.audio_secs();
                let previous = state.resolve_transcript(0).ok();
                log.line(&format!(
                    "parakit: simulating PTT from {} ({audio_secs:.2}s, cycle {cycle}/{repeats})",
                    audio_path.display()
                ));
                let lease = state.activity.begin();
                let started_at = Instant::now();
                let (done_tx, done_rx) = bounded(1);
                tx.send(WorkerEvent::Started {
                    recording: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                })
                .context("send simulated PTT start")?;
                // Immediate release exercises a short PTT while reload is still busy.
                tx.send(WorkerEvent::Stopped {
                    mode: daemon::recording::DictationMode::Plain,
                    started_at,
                    stopped_at: started_at + Duration::from_secs_f32(audio_secs),
                    pcm: wav.samples.clone(),
                    focus_at_start: None,
                    activity: Some(lease),
                    completion: Some(done_tx),
                })
                .context("send simulated PTT stop")?;
                // Inference has no cancellation boundary. A receive timeout would
                // abandon the acknowledgement while the same worker and native
                // session kept running, so wait for completion or disconnection.
                let completed = done_rx
                    .recv()
                    .context("worker disconnected before dictation completion")?;
                if let Err(error) = completed {
                    anyhow::ensure!(
                        state.resolve_transcript(0).ok() == previous,
                        "failed simulated dictation replaced transcript history"
                    );
                    log.verbose("parakit: failed simulated dictation left history unchanged");
                    return Err(anyhow::Error::msg(error));
                }
                let transcript = if config.daemon.effective_transcript_history() == 0 {
                    continue;
                } else {
                    state.resolve_transcript(0)?
                };
                // Same-process parity is a reload invariant, not a cross-build
                // reference-transcript quality gate.
                match &*baseline {
                    Some(expected) if !context_enabled && expected != &transcript => {
                        anyhow::bail!("transcript changed on simulated cycle {cycle}")
                    }
                    None => *baseline = Some(transcript),
                    _ => {}
                }
            }
            if !idle.is_zero() {
                thread::sleep(idle);
                let expected = if timeout.is_some_and(|timeout| idle >= timeout) {
                    Residency::Offloaded
                } else {
                    Residency::Loaded
                };
                // Leave a bounded scheduling margin at the exact timeout boundary.
                let deadline = Instant::now() + Duration::from_secs(2);
                let ready = daemon::poll_ticks(deadline, Duration::from_millis(10)).any(|()| {
                    state
                        .model_status()
                        .is_some_and(|status| status.residency == expected)
                });
                if !ready {
                    anyhow::bail!(
                        "expected model {} after simulated idle, got {:?}",
                        expected.label(),
                        state.model_status()
                    );
                }
            }
        }
        Ok(())
    })();
    drop(tx);
    worker
        .join()
        .map_err(|_| anyhow::anyhow!("PTT simulation worker panicked"))?;
    result
}
