//! Synthetic PCM generation for backend warmup.

use crate::constants::TARGET_RATE;

/// Low nonzero amplitude used for synthetic warmup audio.
pub const SYNTHETIC_AMPLITUDE: f32 = 0.02;

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
