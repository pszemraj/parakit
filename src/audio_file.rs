//! WAV ingestion helpers shared by quality tools and daemon validation paths.

use crate::constants::TARGET_RATE;
use anyhow::{Context, Result};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::path::Path;

const RESAMPLE_CHUNK_SIZE: usize = 1024;

/// Decoded mono WAV audio.
pub struct WavData {
    /// PCM samples mixed to mono.
    pub samples: Vec<f32>,
    /// Source file sample rate.
    pub sample_rate: u32,
}

/// Read a WAV file, normalize samples to `f32`, and mix it to mono.
///
/// # Arguments
///
/// * `path` - WAV file to decode.
///
/// # Returns
///
/// The decoded mono samples and source sample rate.
///
/// # Errors
///
/// Returns an error when the file cannot be read or uses an unsupported sample
/// representation.
pub fn read_wav_mono(path: &Path) -> Result<WavData> {
    let mut reader = hound::WavReader::open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let spec = reader.spec();
    if spec.channels == 0 {
        anyhow::bail!("{} has zero channels", path.display());
    }

    let samples = match spec.sample_format {
        hound::SampleFormat::Float => {
            if spec.bits_per_sample != 32 {
                anyhow::bail!(
                    "unsupported float WAV depth {} in {}",
                    spec.bits_per_sample,
                    path.display()
                );
            }
            reader
                .samples::<f32>()
                .collect::<std::result::Result<Vec<_>, _>>()
                .context("failed to read float WAV samples")?
        }
        hound::SampleFormat::Int => read_int_samples(&mut reader, spec.bits_per_sample)?,
    };

    Ok(WavData {
        samples: mix_to_mono(&samples, spec.channels as usize),
        sample_rate: spec.sample_rate,
    })
}

/// Resample mono PCM to [`TARGET_RATE`].
///
/// # Arguments
///
/// * `samples` - Mono source PCM.
/// * `source_rate` - Sample rate of `samples`.
///
/// # Returns
///
/// Samples at [`TARGET_RATE`].
///
/// # Errors
///
/// Returns an error if the resampler cannot be constructed or run.
///
/// # Panics
///
/// Does not panic.
pub fn resample_to_target(samples: Vec<f32>, source_rate: u32) -> Result<Vec<f32>> {
    if source_rate == TARGET_RATE || samples.is_empty() {
        return Ok(samples);
    }
    if source_rate == 0 {
        anyhow::bail!("source sample rate must be greater than zero");
    }

    let mut resampler = SincFixedIn::<f32>::new(
        TARGET_RATE as f64 / source_rate as f64,
        2.0,
        resampler_params(),
        RESAMPLE_CHUNK_SIZE,
        1,
    )
    .context("failed to construct resampler")?;

    let expected_len =
        (samples.len() as f64 * TARGET_RATE as f64 / source_rate as f64).ceil() as usize;
    let mut output = Vec::with_capacity(expected_len);

    for chunk in samples.chunks(RESAMPLE_CHUNK_SIZE) {
        let mut padded = chunk.to_vec();
        if padded.len() < RESAMPLE_CHUNK_SIZE {
            padded.resize(RESAMPLE_CHUNK_SIZE, 0.0);
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

/// Return the sinc resampler parameters used for file and live audio paths.
///
/// # Returns
///
/// The rubato sinc interpolation parameters.
pub fn resampler_params() -> SincInterpolationParameters {
    SincInterpolationParameters {
        sinc_len: 64,
        f_cutoff: 0.95,
        interpolation: SincInterpolationType::Linear,
        oversampling_factor: 128,
        window: WindowFunction::BlackmanHarris2,
    }
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
                .collect::<std::result::Result<Vec<_>, _>>()
                .context("failed to read 8-bit WAV samples")
        }
        9..=16 => {
            let scale = (1_i32 << (bits_per_sample - 1)) as f32;
            reader
                .samples::<i16>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<std::result::Result<Vec<_>, _>>()
                .context("failed to read 16-bit WAV samples")
        }
        17..=32 => {
            let scale = (1_i64 << (bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 / scale))
                .collect::<std::result::Result<Vec<_>, _>>()
                .context("failed to read 24/32-bit WAV samples")
        }
        other => anyhow::bail!("unsupported integer WAV depth {other}"),
    }
}

fn mix_to_mono(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return samples.to_vec();
    }

    samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().copied().sum::<f32>() / channels as f32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::{SampleFormat, WavSpec, WavWriter};

    #[test]
    fn mixes_stereo_wav_to_mono() {
        let dir = Path::new("target/tmp/audio-file-tests");
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join("stereo.wav");
        let spec = WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut writer = WavWriter::create(&path, spec).unwrap();
        writer.write_sample::<i16>(16_384).unwrap();
        writer.write_sample::<i16>(0).unwrap();
        writer.write_sample::<i16>(0).unwrap();
        writer.write_sample::<i16>(-16_384).unwrap();
        writer.finalize().unwrap();

        let wav = read_wav_mono(&path).unwrap();
        assert_eq!(wav.sample_rate, 48_000);
        assert_eq!(wav.samples.len(), 2);
        assert!((wav.samples[0] - 0.25).abs() < 0.001);
        assert!((wav.samples[1] + 0.25).abs() < 0.001);
    }

    #[test]
    fn resample_to_target_preserves_empty_and_target_rate_input() {
        assert!(resample_to_target(Vec::new(), 48_000).unwrap().is_empty());
        assert_eq!(
            resample_to_target(vec![0.1, -0.1], TARGET_RATE).unwrap(),
            vec![0.1, -0.1]
        );
    }
}
