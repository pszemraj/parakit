//! Shared model readiness and synthetic Parakeet warmup.

use crate::constants::TARGET_RATE;
use crate::inference::Engine;
use anyhow::{Context, Result};
use std::io::Cursor;

// Human speech from JFK's public-domain inaugural address (20 January 1961).
// https://www.jfklibrary.org/asset-viewer/archives/jfkwha-001
// Embed the pinned vendor sample so installed binaries need no audio asset path.
const QWEN_SPEECH: &[u8] = include_bytes!("../vendor/CrispASR/bindings/go/samples/jfk.wav");

/// Run readiness inference without returning its text.
///
/// # Arguments
///
/// * `engine` - Loaded engine with the user's resolved inference options.
/// * `synthetic_seconds` - Parakeet warmup shapes; Qwen always uses speech.
///
/// # Returns
/// Success after the probe completes and the user's options are restored.
///
/// # Errors
///
/// Reports an unreadable embedded fixture, inference failure, or failure to
/// restore Qwen's user options. Qwen temporarily uses automatic language and
/// 512 tokens, independently of the user's language and generation budget.
pub fn run(engine: &Engine, synthetic_seconds: &[usize]) -> Result<()> {
    if engine.backend() == "qwen3" {
        engine.transcribe_readiness(&qwen_speech_pcm()?)
    } else {
        for seconds in synthetic_seconds {
            engine.transcribe(&synthetic_pcm(*seconds))?;
        }
        Ok(())
    }
}

fn qwen_speech_pcm() -> Result<Vec<f32>> {
    hound::WavReader::new(Cursor::new(QWEN_SPEECH))
        .context("read embedded Qwen readiness speech")?
        .samples::<i16>()
        .map(|sample| {
            sample
                .map(|value| f32::from(value) / 32768.0)
                .map_err(Into::into)
        })
        .collect()
}

const CPU_ENGINE_WARMUP_SECONDS: &[usize] = &[1];
// Bound GPU startup readiness work without reserving a longer recording's
// workspace. Longer first recordings grow it on demand.
const GPU_ENGINE_WARMUP_SECONDS: &[usize] = &[5];

/// Return the startup readiness warmup shapes for the requested device policy.
///
/// # Arguments
///
/// * `device_mode` - Requested CPU/automatic/GPU selection.
/// * `has_gpu` - Whether the runtime probe sees a usable GPU.
///
/// # Returns
///
/// One second for CPU, or five seconds for a visible GPU.
pub fn engine_warmup_seconds(
    device_mode: crate::inference::DeviceMode,
    has_gpu: bool,
) -> &'static [usize] {
    if device_mode != crate::inference::DeviceMode::Cpu && has_gpu {
        GPU_ENGINE_WARMUP_SECONDS
    } else {
        CPU_ENGINE_WARMUP_SECONDS
    }
}

/// Return the readiness probe used when reopening a session in the same process.
///
/// Backend initialization has already run at startup. A short inference still
/// verifies the new session before queued user audio is processed, without
/// precomputing larger shapes on every reload.
///
/// # Returns
///
/// One second of synthetic input, on every backend.
pub fn reload_warmup_seconds() -> &'static [usize] {
    CPU_ENGINE_WARMUP_SECONDS
}

/// Low nonzero amplitude used for synthetic warmup audio.
const SYNTHETIC_AMPLITUDE: f32 = 0.02;

/// Build deterministic non-silent mono PCM at the model sample rate.
///
/// # Arguments
///
/// * `seconds` - Synthetic input length in seconds.
///
/// # Returns
///
/// A mono PCM buffer at [`TARGET_RATE`].
///
/// # Panics
///
/// Panics if allocating the synthetic PCM buffer fails.
pub fn synthetic_pcm(seconds: usize) -> Vec<f32> {
    let sample_count = TARGET_RATE as usize * seconds;
    (0..sample_count)
        .map(|index| {
            if (index / 80) % 2 == 0 {
                SYNTHETIC_AMPLITUDE
            } else {
                -SYNTHETIC_AMPLITUDE
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_readiness_is_eleven_seconds_of_mono_16khz_speech() {
        let reader = hound::WavReader::new(Cursor::new(QWEN_SPEECH)).unwrap();
        assert_eq!(reader.spec().sample_rate, TARGET_RATE);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().bits_per_sample, 16);
        let pcm = qwen_speech_pcm().unwrap();
        assert_eq!(pcm.len(), TARGET_RATE as usize * 11);
        assert!(pcm.iter().any(|sample| sample.abs() > 0.1));
    }

    #[test]
    fn synthetic_pcm_is_representative_and_nonzero() {
        let pcm = synthetic_pcm(30);

        assert_eq!(pcm.len(), TARGET_RATE as usize * 30);
        assert!(pcm.iter().any(|sample| *sample > 0.0));
        assert!(pcm.iter().any(|sample| *sample < 0.0));
        assert!(pcm.iter().all(|sample| sample.abs() <= SYNTHETIC_AMPLITUDE));
    }

    #[test]
    fn startup_and_reload_shapes_stay_distinct() {
        use crate::inference::DeviceMode;

        assert_eq!(engine_warmup_seconds(DeviceMode::Cpu, false), &[1]);
        assert_eq!(engine_warmup_seconds(DeviceMode::Gpu, true), &[5]);
        assert_eq!(reload_warmup_seconds(), &[1]);
    }
}
