//! Synthetic PCM generation for backend warmup.

use crate::constants::TARGET_RATE;

const CPU_ENGINE_WARMUP_SECONDS: &[usize] = &[1];
// Cover the common short-dictation shape without making startup reserve a
// longer recording's workspace. Longer first recordings grow it on demand.
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
