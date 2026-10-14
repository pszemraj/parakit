//! Transcribe one WAV file through parakit's real CrispASR engine.
//!
//! This maintainer tool is registered as a Cargo example target so it is easy
//! to run during validation without being installed as an end-user binary.
//! It prints raw inference output only; daemon text-cleaning rules are not run.

use anyhow::{Context, Result};
use clap::Parser;
use parakit::audio_file::prepare_wav_for_model;
use parakit::fetch;
use parakit::gguf;
use parakit::inference::{default_thread_count, DeviceMode, Engine};
use parakit::model::{resolve_model_selection, ModelPreset, ModelSelection};
use parakit::qwen::{normalize_qwen_language, QwenOptions};
use parakit::warmup;
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
    #[arg(short = 'm', long, conflicts_with = "model_preset")]
    model: Option<PathBuf>,

    /// Hosted Q8_0 model preset; defaults to parakeet-v3. Qwen is experimental.
    #[arg(long, value_enum)]
    model_preset: Option<ModelPreset>,

    /// Qwen source language code/name, or auto (default).
    #[arg(long)]
    qwen_language: Option<String>,

    /// Qwen generation budget per chunk (1 to 4096; default 512).
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=4096))]
    qwen_max_new_tokens: Option<u32>,

    /// Path to a WAV file. The tool mixes to mono and resamples to 16 kHz.
    #[arg(short = 'a', long)]
    audio: PathBuf,

    /// CPU inference threads. Defaults to half the logical CPUs, at least 2 (1 on a single-CPU machine).
    #[arg(long, value_name = "N")]
    threads: Option<NonZeroUsize>,

    /// Runtime compute device. `auto` uses the best GPU when available and CPU otherwise.
    #[arg(long, value_enum, default_value = "auto")]
    device: DeviceMode,

    /// Repeat inference on the same loaded model for timing comparisons.
    #[arg(long, default_value = "1")]
    repeat: NonZeroUsize,

    /// Run the same model-aware readiness probe as daemon startup/reload.
    #[arg(long, conflicts_with = "warmup_seconds")]
    warmup: bool,

    /// Run one synthetic warmup pass before timing the real clip. Repeat for multiple passes.
    #[arg(long, value_name = "SECONDS")]
    warmup_seconds: Vec<usize>,
}

fn main() -> Result<()> {
    // Apply environment defaults before starting threads or opening a model.
    unsafe { parakit::inference::initialize_crispasr_runtime_defaults() };
    let cli = Cli::parse();
    let qwen = QwenOptions {
        language: cli
            .qwen_language
            .as_deref()
            .map(normalize_qwen_language)
            .transpose()
            .map_err(anyhow::Error::msg)?
            .flatten(),
        max_new_tokens: cli.qwen_max_new_tokens.unwrap_or(512) as usize,
    };
    qwen.validate().map_err(anyhow::Error::msg)?;
    let explicit_qwen = cli.qwen_language.is_some() || cli.qwen_max_new_tokens.is_some();
    let selection = resolve_model_selection(cli.model.as_deref(), cli.model_preset, None, None)?;
    if explicit_qwen
        && matches!(&selection, ModelSelection::Preset(preset) if preset.backend() != "qwen3")
    {
        anyhow::bail!("Qwen controls require a Qwen3-ASR model");
    }

    let wav = prepare_wav_for_model(&cli.audio)?;
    let original_rate = wav.source_rate;
    let audio_secs = wav.audio_secs();

    let model_path = match selection {
        ModelSelection::Local(path) => path,
        ModelSelection::Preset(preset) => {
            fetch::ensure_preset_model_with_verbosity(preset, false, false)?
        }
    };
    if explicit_qwen {
        parakit::model::validate_model_file(&model_path)?;
        let model_str = model_path
            .to_str()
            .context("model path is not valid UTF-8")?;
        let backend = crispasr::Session::detect_backend(model_str).map_err(anyhow::Error::msg)?;
        if backend != "qwen3" {
            anyhow::bail!("Qwen controls require a Qwen3-ASR model (detected {backend})");
        }
    }
    let threads = cli
        .threads
        .map(NonZeroUsize::get)
        .unwrap_or_else(default_thread_count);
    let engine = Engine::open_with_options(&model_path, threads, cli.device, &qwen)
        .with_context(|| format!("could not open model {}", model_path.display()))?;
    let dtype = gguf::dtype_label(&model_path);
    if engine.backend() == "qwen3" && !cli.warmup_seconds.is_empty() {
        anyhow::bail!("--warmup-seconds is synthetic Parakeet warmup; use --warmup for Qwen");
    }

    println!("model:   {}", model_path.display());
    println!("dtype:   {dtype}");
    println!("backend: {}", engine.backend());
    println!("threads: {}", engine.threads());
    println!("device:  {}", engine.device_mode().as_str());
    println!("audio:   {:.2}s", audio_secs);
    println!("source:  {} Hz", original_rate);

    if cli.warmup {
        #[cfg(feature = "bundled")]
        let has_gpu = parakit::gpu::has_gpu_device();
        #[cfg(not(feature = "bundled"))]
        let has_gpu = false;
        let started = Instant::now();
        warmup::run(&engine, warmup::engine_warmup_seconds(cli.device, has_gpu))?;
        println!("warmup: {:.0}ms", started.elapsed().as_secs_f32() * 1000.0);
    }

    for seconds in &cli.warmup_seconds {
        let warmup_pcm = warmup::synthetic_pcm(*seconds);
        let started = Instant::now();
        engine.transcribe(&warmup_pcm)?;
        let elapsed = started.elapsed();
        println!("warmup_audio: {seconds}s");
        println!("warmup: {:.0}ms", elapsed.as_secs_f32() * 1000.0);
    }

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
        let cases = [
            (10.0, Duration::from_millis(2_000), 0.2, 5.0),
            (0.0, Duration::from_millis(2_000), f32::INFINITY, 0.0),
            (10.0, Duration::ZERO, 0.0, f32::INFINITY),
        ];

        for (audio_secs, infer, expected_rtf, expected_speed) in cases {
            assert_metric_eq(real_time_factor(audio_secs, infer), expected_rtf);
            assert_metric_eq(realtime_speed(audio_secs, infer), expected_speed);
        }
    }

    fn assert_metric_eq(actual: f32, expected: f32) {
        if expected.is_infinite() {
            assert_eq!(actual, expected);
        } else {
            assert!((actual - expected).abs() < f32::EPSILON);
        }
    }
}
