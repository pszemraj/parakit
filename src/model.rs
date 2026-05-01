//! Canonical Parakeet model names and cache paths.

use anyhow::{Context, Result};
use directories::BaseDirs;
use std::path::PathBuf;

/// Direct download URL for the official `.nemo` checkpoint.
pub const OFFICIAL_NEMO_URL: &str =
    "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3/resolve/main/parakeet-tdt-0.6b-v3.nemo";
/// Direct download URL for the default hosted Q8_0 GGUF.
pub const HOSTED_Q8_URL: &str = "https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf/resolve/main/parakeet-tdt-0.6b-v3-Q8_0.gguf";
/// Expected SHA256 for the default hosted Q8_0 GGUF.
pub const HOSTED_Q8_SHA256: &str =
    "10f38dd9ce69ce555a413d9b4201ae5d93c2d7cadc91a285f4bfeeec6eee635a";
/// File name for the downloaded official NeMo checkpoint.
pub const NEMO_FILENAME: &str = "parakeet-tdt-0.6b-v3.nemo";
/// File name for the intermediate F16 GGUF.
pub const F16_FILENAME: &str = "parakeet-tdt-0.6b-v3-F16.gguf";
/// File name for the canonical Q8_0 GGUF used by parakit by default.
pub const Q8_FILENAME: &str = "parakeet-tdt-0.6b-v3-Q8_0.gguf";
/// File name for model acquisition metadata.
pub const MANIFEST_FILENAME: &str = "manifest.json";

/// Return the platform cache directory that holds parakit model files.
///
/// # Returns
///
/// The directory where model artifacts and `manifest.json` are stored.
///
/// # Errors
///
/// Returns an error if the operating system does not expose a usable user
/// cache or local-data directory.
pub fn models_dir() -> Result<PathBuf> {
    let dirs = BaseDirs::new().context("could not determine user cache directory")?;

    #[cfg(target_os = "windows")]
    {
        Ok(dirs
            .data_local_dir()
            .join("parakit")
            .join("Cache")
            .join("models"))
    }

    #[cfg(not(target_os = "windows"))]
    {
        Ok(dirs.cache_dir().join("parakit").join("models"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_model_constants_are_consistent() {
        assert_eq!(Q8_FILENAME, "parakeet-tdt-0.6b-v3-Q8_0.gguf");
        assert_eq!(F16_FILENAME, "parakeet-tdt-0.6b-v3-F16.gguf");
        assert!(OFFICIAL_NEMO_URL.ends_with(NEMO_FILENAME));
        assert!(HOSTED_Q8_URL.ends_with(Q8_FILENAME));
        assert_eq!(HOSTED_Q8_SHA256.len(), 64);
    }
}
