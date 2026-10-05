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
/// * `log` - Shared process logger.
/// * `audio_path` - Real WAV fixture to replay.
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
    log: Arc<Logger>,
    audio_path: &Path,
) -> Result<()> {
    let verbose = cli.effective_verbose(config);
    let cleaner = build_cli_cleaner(start, config)?.map(Arc::new);
    let data_log = start
        .effective_log_dir(config)
        .map(|dir| Arc::new(DataLogger::new(dir)));
    let minutes = start.effective_model_idle_minutes(config);
    let timeout = idle_timeout(minutes)?;
    let idle = Duration::from_secs(start.simulate_ptt_idle_seconds.unwrap_or(0));
    let repeats = start.simulate_ptt_repeat.map_or(1, NonZeroUsize::get);
    // Validate before model initialization rather than overflowing a sleep later.
    if Instant::now().checked_add(idle).is_none() {
        anyhow::bail!("simulated idle interval exceeds the supported clock range");
    }
    let prepare_started = Instant::now();
    let wav = prepare_wav_for_model(audio_path)?;
    let audio_secs = wav.audio_secs();
    log.verbose(format!(
        "parakit: simulated audio prepared in {:.0}ms (source_rate={} Hz, source_samples={}, target_samples={})",
        prepare_started.elapsed().as_secs_f32() * 1000.0,
        wav.source_rate, wav.source_samples, wav.samples.len()
    ));
    let OpenedEngine { engine, recipe, .. } =
        open_cli_engine(start, config, verbose, cli.quiet || !verbose, &log)?;
    let state = Arc::new(daemon::ipc::SharedState::new());
    let worker_lifetime = state.shutdown.register();
    let (tx, rx) = bounded::<WorkerEvent>(WORKER_QUEUE_CAPACITY);
    let worker = spawn_worker(WorkerCtx {
        engine,
        recipe,
        model_idle_minutes: minutes,
        cleaner,
        data_log,
        sounds: Sounds::new(false),
        log: Arc::clone(&log),
        notifier: Notifier::new(Arc::new(Logger::new(LogLevel::Quiet))),
        state: Arc::clone(&state),
        paste_mode: start.effective_paste_mode(config),
        keep_transcript_clipboard: start.effective_keep_transcript_clipboard(config),
        insert_transcripts: false,
        rx,
        lifetime: worker_lifetime,
    });
    state.activity.ready();
    let result = (|| {
        let mut baseline = None;
        for cycle in 1..=repeats {
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
                started_at,
                stopped_at: started_at + Duration::from_secs_f32(audio_secs),
                pcm: wav.samples.clone(),
                focus_at_start: None,
                activity: Some(lease),
                completion: Some(done_tx),
            })
            .context("send simulated PTT stop")?;
            done_rx
                .recv()
                .context("worker disconnected before dictation completion")?
                .map_err(anyhow::Error::msg)?;
            let transcript = state.resolve_transcript(0)?;
            // Same-process parity is a reload invariant, not a cross-build
            // reference-transcript quality gate.
            match &baseline {
                Some(expected) if expected != &transcript => {
                    anyhow::bail!("transcript changed on simulated cycle {cycle}")
                }
                None => baseline = Some(transcript),
                _ => {}
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
                loop {
                    if state
                        .model_status()
                        .is_some_and(|status| status.residency == expected)
                    {
                        break;
                    }
                    if Instant::now() >= deadline {
                        anyhow::bail!(
                            "expected model {} after simulated idle, got {:?}",
                            expected.label(),
                            state.model_status()
                        );
                    }
                    thread::sleep(Duration::from_millis(10));
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
