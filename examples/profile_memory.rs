//! Repeated real Engine sessions with stable checkpoints for native memory tools.
//!
//! This measures the native allocation lifecycle, not hotkeys or insertion. Use
//! the daemon's WAV simulation separately to validate worker timeout behavior.

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use parakit::audio_file::prepare_wav_for_model;
use parakit::inference::{DeviceMode, Engine};
use parakit::warmup;
use sha2::{Digest, Sha256};
use std::io::{self, Write};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum ReloadWarmup {
    #[default]
    Production,
    Startup,
    OneSecond,
    None,
}

impl ReloadWarmup {
    fn sequence(self, device: DeviceMode, has_gpu: bool, cycle: usize) -> &'static [usize] {
        // Preserve startup initialization so comparisons isolate same-process reloads.
        match (cycle, self) {
            (1, _) | (_, Self::Startup) => warmup::engine_warmup_seconds(device, has_gpu),
            (_, Self::Production) => warmup::reload_warmup_seconds(),
            (_, Self::OneSecond) => &[1],
            (_, Self::None) => &[],
        }
    }
}

#[derive(Parser)]
struct Cli {
    /// Local GGUF file; this tool never downloads a model.
    #[arg(long)]
    model: PathBuf,
    /// Real WAV used for the short and full transcription checkpoints.
    #[arg(long)]
    audio: PathBuf,
    /// Requested inference device.
    #[arg(long, value_enum, default_value = "auto")]
    device: DeviceMode,
    /// CPU inference threads.
    #[arg(long, default_value = "8")]
    threads: NonZeroUsize,
    /// Number of open/transcribe/close cycles.
    #[arg(long, default_value = "10")]
    cycles: NonZeroUsize,
    /// Measure the resident baseline instead of closing between cycles.
    #[arg(long)]
    keep_loaded: bool,
    /// Wait for one stdin line after each JSON checkpoint while a sampler runs.
    #[arg(long)]
    wait_for_sampler: bool,
    /// Experimental reload warmup; the first session always uses startup policy.
    #[arg(
        long,
        value_enum,
        default_value = "production",
        conflicts_with = "keep_loaded"
    )]
    reload_warmup: ReloadWarmup,
    /// Measure a full recording as the first real inference after every open.
    #[arg(long)]
    full_first: bool,
}

fn checkpoint(
    cli: &Cli,
    cycle: usize,
    phase: &str,
    elapsed_ms: f64,
    transcript: Option<&str>,
) -> Result<()> {
    println!(
        "{}",
        serde_json::json!({
            "pid": std::process::id(), "cycle": cycle, "phase": phase,
            "elapsed_ms": elapsed_ms, "device": cli.device.as_str(),
            "threads": cli.threads.get(), "keep_loaded": cli.keep_loaded,
            "reload_warmup": format!("{:?}", cli.reload_warmup),
            "full_first": cli.full_first,
            "transcript_sha256": transcript.map(|text| format!("{:x}", Sha256::digest(text))),
        })
    );
    io::stdout().flush()?;
    if cli.wait_for_sampler {
        let mut ack = String::new();
        if io::stdin().read_line(&mut ack)? == 0 {
            bail!("memory sampler disconnected");
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let wav = prepare_wav_for_model(&cli.audio)?;
    checkpoint(&cli, 0, "before_load", 0.0, None)?;
    #[cfg(feature = "bundled")]
    if cli.device == DeviceMode::Gpu && !parakit::gpu::has_gpu_device() {
        bail!("GPU requested, but no GPU is available");
    }
    let mut engine = None;
    let mut short_baseline = None;
    let mut full_baseline = None;
    for cycle in 1..=cli.cycles.get() {
        if engine.is_none() {
            let started = Instant::now();
            let opened = Engine::open(&cli.model, cli.threads.get(), cli.device)?;
            checkpoint(
                &cli,
                cycle,
                "after_load",
                started.elapsed().as_secs_f64() * 1000.0,
                None,
            )?;
            #[cfg(feature = "bundled")]
            let has_gpu = cli.device != DeviceMode::Cpu && parakit::gpu::has_gpu_device();
            #[cfg(not(feature = "bundled"))]
            let has_gpu = false;
            let started = Instant::now();
            for seconds in cli.reload_warmup.sequence(cli.device, has_gpu, cycle) {
                opened.transcribe(&warmup::synthetic_pcm(*seconds))?;
            }
            checkpoint(
                &cli,
                cycle,
                "after_warmup",
                started.elapsed().as_secs_f64() * 1000.0,
                None,
            )?;
            engine = Some(opened);
        }
        let session = engine.as_ref().context("session should be loaded")?;
        let short = &wav.samples[..wav.samples.len().min(2 * 16000)];
        let mut clips = [
            ("after_short", short, &mut short_baseline),
            ("after_full", wav.samples.as_slice(), &mut full_baseline),
        ];
        if cli.full_first {
            clips.reverse();
        }
        for (phase, pcm, baseline) in clips {
            let started = Instant::now();
            let text = session.transcribe(pcm)?;
            let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
            if text.trim().is_empty() {
                bail!("empty {phase} transcript at cycle {cycle}");
            }
            match baseline {
                Some(expected) if expected != &text => {
                    bail!("{phase} transcript changed at cycle {cycle}")
                }
                None => *baseline = Some(text.clone()),
                _ => {}
            }
            checkpoint(&cli, cycle, phase, elapsed_ms, Some(&text))?;
        }
        let started = Instant::now();
        if !cli.keep_loaded {
            drop(engine.take());
        }
        checkpoint(
            &cli,
            cycle,
            if cli.keep_loaded {
                "retained"
            } else {
                "offloaded"
            },
            started.elapsed().as_secs_f64() * 1000.0,
            None,
        )?;
    }
    drop(engine.take());
    checkpoint(&cli, cli.cycles.get(), "closed", 0.0, None)
}
