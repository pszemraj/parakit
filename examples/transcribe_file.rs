//! Transcribe one WAV file through parakit's real CrispASR engine.
//!
//! This maintainer tool is registered as a Cargo example target so it is easy
//! to run during validation without being installed as an end-user binary.
//! It prints raw inference output only; daemon text-cleaning rules are not run.

use anyhow::{Context, Result};
use clap::Parser;
use parakit::audio_file::{read_wav_mono, resample_to_target};
use parakit::constants::TARGET_RATE;
use parakit::fetch;
use parakit::gguf;
use parakit::inference::{default_thread_count, DeviceMode, Engine};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(
    name = "transcribe-file",
    about = "Transcribe a WAV file through parakit's CrispASR engine."
)]
struct Cli {
    /// Path to a GGUF model file. Overrides the cached Q8_0 model.
    #[arg(short = 'm', long)]
    model: Option<PathBuf>,

    /// Path to a WAV file. The tool mixes to mono and resamples to 16 kHz.
    #[arg(short = 'a', long)]
    audio: PathBuf,

    /// CPU inference threads. Defaults to a conservative detected count.
    #[arg(long, value_name = "N")]
    threads: Option<NonZeroUsize>,

    /// Runtime compute device. `auto` uses the best GPU when available and CPU otherwise.
    #[arg(long, value_enum, default_value = "auto")]
    device: DeviceMode,

    /// Repeat inference on the same loaded model for timing comparisons.
    #[arg(long, default_value = "1")]
    repeat: NonZeroUsize,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let mut wav = read_wav_mono(&cli.audio)?;
    let original_rate = wav.sample_rate;
    wav.samples = resample_to_target(wav.samples, original_rate)?;
    let audio_secs = wav.samples.len() as f32 / TARGET_RATE as f32;

    let model_path = match cli.model.as_deref() {
        Some(path) => path.to_path_buf(),
        None => fetch::ensure_default_model(false)?,
    };
    let threads = cli
        .threads
        .map(NonZeroUsize::get)
        .unwrap_or_else(default_thread_count);
    let engine = Engine::open(&model_path, threads, cli.device)
        .with_context(|| format!("could not open model {}", model_path.display()))?;
    let dtype = gguf::dtype_label(&model_path);

    println!("model:   {}", model_path.display());
    println!("dtype:   {dtype}");
    println!("backend: {}", engine.backend());
    println!("threads: {}", engine.threads());
    println!("device:  {}", engine.device_mode().as_str());
    println!("audio:   {:.2}s", audio_secs);
    println!("source:  {} Hz", original_rate);

    let mut timings = Vec::with_capacity(cli.repeat.get());
    for idx in 1..=cli.repeat.get() {
        let started = Instant::now();
        let raw = engine.transcribe(&wav.samples)?;
        let infer = started.elapsed();
        timings.push(infer);

        if cli.repeat.get() > 1 {
            println!("run:     {idx}/{}", cli.repeat);
        }
        println!("infer:   {:.0}ms", infer.as_secs_f32() * 1000.0);
        println!("rtf:     {:.2}x", real_time_factor(audio_secs, infer));
        println!(
            "speed:   {:.2}x realtime",
            realtime_speed(audio_secs, infer)
        );
        println!("Raw:     {}", raw);
    }

    if timings.len() > 1 {
        let avg_ms =
            timings.iter().map(Duration::as_secs_f32).sum::<f32>() * 1000.0 / timings.len() as f32;
        println!("avg:     {avg_ms:.0}ms");
    }

    Ok(())
}

fn real_time_factor(audio_secs: f32, infer: Duration) -> f32 {
    if audio_secs <= 0.0 {
        return f32::INFINITY;
    }
    infer.as_secs_f32() / audio_secs
}

fn realtime_speed(audio_secs: f32, infer: Duration) -> f32 {
    if infer.is_zero() {
        return f32::INFINITY;
    }
    audio_secs / infer.as_secs_f32()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rtf_is_inference_time_over_audio_duration() {
        let infer = Duration::from_millis(2_000);

        assert!((real_time_factor(10.0, infer) - 0.2).abs() < f32::EPSILON);
        assert!((realtime_speed(10.0, infer) - 5.0).abs() < f32::EPSILON);
    }
}
