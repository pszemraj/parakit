//! Transcribe one WAV file through parakit's real CrispASR engine.
//!
//! This helper is used for quality checks against other Parakeet
//! implementations without starting the hotkey daemon.

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use parakit::constants::TARGET_RATE;
use parakit::fetch;
use parakit::gguf;
use parakit::inference::default_thread_count;
use parakit::inference::Engine;
use parakit::model;
use parakit::rules::{self, Cleaner};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
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

    /// CPU inference threads. Defaults to the OS available parallelism.
    #[arg(long, value_name = "N")]
    threads: Option<NonZeroUsize>,

    /// Repeat inference on the same loaded model for timing comparisons.
    #[arg(long, default_value = "1")]
    repeat: NonZeroUsize,

    /// Disable all text cleaning rules.
    #[arg(long)]
    no_cleaning: bool,

    /// Disable a specific rule by name. Repeatable.
    #[arg(long, value_name = "NAME")]
    disable_rule: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let disabled: HashSet<String> = cli.disable_rule.iter().cloned().collect();
    for name in &cli.disable_rule {
        rules::assert_rule_name_exists(name)?;
    }
    let cleaner = if cli.no_cleaning {
        None
    } else {
        Some(Cleaner::new(&disabled)?)
    };

    let mut wav = read_wav_mono(&cli.audio)?;
    let original_rate = wav.sample_rate;
    wav.samples = resample_to_target(wav.samples, original_rate)?;
    let audio_secs = wav.samples.len() as f32 / TARGET_RATE as f32;

    let model_path = match cli.model.as_deref() {
        Some(path) => model::resolve_model_path(Some(path))?,
        None => fetch::ensure_default_model(false)?,
    };
    let threads = cli
        .threads
        .map(NonZeroUsize::get)
        .unwrap_or_else(default_thread_count);
    let engine = Engine::open_with_threads(&model_path, threads)
        .with_context(|| format!("could not open model {}", model_path.display()))?;
    let dtype = gguf::detect_dtype(&model_path)
        .ok()
        .flatten()
        .unwrap_or_else(|| "unknown".to_string());

    println!("model:   {}", model_path.display());
    println!("dtype:   {dtype}");
    println!("backend: {}", engine.backend());
    println!("threads: {}", engine.threads());
    println!("audio:   {:.2}s", audio_secs);
    println!("source:  {} Hz", original_rate);

    let mut timings = Vec::with_capacity(cli.repeat.get());
    for idx in 1..=cli.repeat.get() {
        let started = Instant::now();
        let raw = engine.transcribe(&wav.samples)?;
        let infer = started.elapsed();
        timings.push(infer);
        let cleaned = cleaner
            .as_ref()
            .map_or_else(|| raw.clone(), |c| c.clean(&raw));

        if cli.repeat.get() > 1 {
            println!("run:     {idx}/{}", cli.repeat);
        }
        println!("infer:   {:.0}ms", infer.as_secs_f32() * 1000.0);
        println!("rtf:     {:.2}x", realtime_factor(audio_secs, infer));
        println!("Raw:     {}", raw);
        println!("Clean:   {}", cleaned);
    }

    if timings.len() > 1 {
        let avg_ms =
            timings.iter().map(Duration::as_secs_f32).sum::<f32>() * 1000.0 / timings.len() as f32;
        println!("avg:     {avg_ms:.0}ms");
    }

    Ok(())
}

fn realtime_factor(audio_secs: f32, infer: Duration) -> f32 {
    if infer.is_zero() {
        return f32::INFINITY;
    }
    audio_secs / infer.as_secs_f32()
}

struct WavData {
    samples: Vec<f32>,
    sample_rate: u32,
}

fn read_wav_mono(path: &Path) -> Result<WavData> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let spec = reader.spec();
    if spec.channels == 0 {
        return Err(anyhow!("{} has zero channels", path.display()));
    }

    let samples = match spec.sample_format {
        hound::SampleFormat::Float => {
            if spec.bits_per_sample != 32 {
                return Err(anyhow!(
                    "unsupported float WAV depth {} in {}",
                    spec.bits_per_sample,
                    path.display()
                ));
            }
            reader
                .samples::<f32>()
                .collect::<Result<Vec<_>, _>>()
                .context("failed to read float WAV samples")?
        }
        hound::SampleFormat::Int => read_int_samples(&mut reader, spec.bits_per_sample)?,
    };

    Ok(WavData {
        samples: mix_to_mono(&samples, spec.channels as usize),
        sample_rate: spec.sample_rate,
    })
}

fn read_int_samples<R: std::io::Read>(
    reader: &mut hound::WavReader<R>,
    bits_per_sample: u16,
) -> Result<Vec<f32>> {
    match bits_per_sample {
        1..=8 => {
            let scale = (1_i32 << (bits_per_sample - 1)) as f32;
            reader
                .samples::<i8>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<Vec<_>, _>>()
                .context("failed to read 8-bit WAV samples")
        }
        9..=16 => {
            let scale = (1_i32 << (bits_per_sample - 1)) as f32;
            reader
                .samples::<i16>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<Vec<_>, _>>()
                .context("failed to read 16-bit WAV samples")
        }
        17..=32 => {
            let scale = (1_i64 << (bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<Result<Vec<_>, _>>()
                .context("failed to read 24/32-bit WAV samples")
        }
        other => Err(anyhow!("unsupported integer WAV depth {other}")),
    }
}

fn mix_to_mono(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return samples.to_vec();
    }

    let frames = samples.len() / channels;
    let mut mono = Vec::with_capacity(frames);
    for frame in samples.chunks_exact(channels) {
        let sum: f32 = frame.iter().copied().sum();
        mono.push(sum / channels as f32);
    }
    mono
}

fn resample_to_target(samples: Vec<f32>, source_rate: u32) -> Result<Vec<f32>> {
    if source_rate == TARGET_RATE {
        return Ok(samples);
    }
    if samples.is_empty() {
        return Ok(samples);
    }

    let params = SincInterpolationParameters {
        sinc_len: 128,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 256,
        window: WindowFunction::BlackmanHarris2,
    };
    let chunk_size = 1024;
    let mut resampler = SincFixedIn::<f32>::new(
        TARGET_RATE as f64 / source_rate as f64,
        2.0,
        params,
        chunk_size,
        1,
    )
    .context("failed to construct resampler")?;

    let expected_len =
        (samples.len() as f64 * TARGET_RATE as f64 / source_rate as f64).ceil() as usize;
    let mut output = Vec::with_capacity(expected_len);

    for chunk in samples.chunks(chunk_size) {
        let mut padded = chunk.to_vec();
        if padded.len() < chunk_size {
            padded.resize(chunk_size, 0.0);
        }
        let input_frames = vec![padded];
        let output_frames = resampler
            .process(&input_frames, None)
            .context("failed to resample WAV chunk")?;
        if let Some(ch0) = output_frames.first() {
            output.extend_from_slice(ch0);
        }
    }

    output.truncate(expected_len.min(output.len()));
    Ok(output)
}
