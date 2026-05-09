//! Canonical Parakeet model names and cache paths.

use anyhow::{Context, Result};
use directories::BaseDirs;
use std::path::PathBuf;

/// Environment variable that overrides the platform model cache directory.
pub const MODELS_DIR_ENV: &str = "PARAKIT_MODELS_DIR";
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
    if let Some(path) = override_models_dir()? {
        return Ok(path);
    }

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

fn override_models_dir() -> Result<Option<PathBuf>> {
    let Some(raw) = std::env::var_os(MODELS_DIR_ENV) else {
        return Ok(None);
    };
    if raw.is_empty() {
        anyhow::bail!("{MODELS_DIR_ENV} is set but empty");
    }
    Ok(Some(PathBuf::from(raw)))
}
