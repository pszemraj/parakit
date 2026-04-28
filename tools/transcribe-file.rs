use anyhow::{anyhow, Context, Result};
use clap::Parser;
use parakit::constants::TARGET_RATE;
use parakit::inference::Engine;
use parakit::rules::{self, Cleaner};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser, Debug)]
#[command(
    name = "transcribe-file",
    about = "Transcribe a WAV file through parakit's CrispASR engine."
)]
struct Cli {
    /// Path to the GGUF model file.
    #[arg(short = 'm', long)]
    model: PathBuf,

    /// Path to a WAV file. The tool mixes to mono and resamples to 16 kHz.
    #[arg(short = 'a', long)]
    audio: PathBuf,

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

    let engine = Engine::open(&cli.model)
        .with_context(|| format!("could not open model {}", cli.model.display()))?;

    let started = Instant::now();
    let raw = engine.transcribe(&wav.samples)?;
    let infer = started.elapsed();
    let cleaned = cleaner
        .as_ref()
        .map_or_else(|| raw.clone(), |c| c.clean(&raw));

    println!("audio:   {:.2}s", audio_secs);
    println!("source:  {} Hz", original_rate);
    println!("infer:   {:.0}ms", infer.as_secs_f32() * 1000.0);
    println!("Raw:     {}", raw);
    println!("Clean:   {}", cleaned);

    Ok(())
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
