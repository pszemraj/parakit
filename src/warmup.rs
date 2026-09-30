//! Synthetic PCM generation for backend warmup.

use crate::constants::TARGET_RATE;

const CPU_ENGINE_WARMUP_SECONDS: &[usize] = &[1];
// The daemon hard-stops held recordings at MAX_UTTERANCE_SECONDS, but warming
// that full 270s shape would make every launch pay worst-case compute. This is
// a realistic-latency policy: cover short dictations and normal 2-25s
// dictations with margin, accepting a one-time backend stall for unusual longer
// cold-cache captures.
const GPU_ENGINE_WARMUP_SECONDS: &[usize] = &[5, 30];

/// Return the daemon readiness warmup shapes for the requested device policy.
///
/// # Arguments
///
/// * `device_mode` - Requested CPU/automatic/GPU selection.
/// * `has_gpu` - Whether the runtime probe sees a usable GPU.
///
/// # Returns
///
/// One second for CPU, or five and thirty seconds for a visible GPU.
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
}
