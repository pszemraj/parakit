//! Canonical Parakeet model names, cache paths, and model path resolution.

use anyhow::{Context, Result};
use directories::BaseDirs;
use std::path::{Path, PathBuf};

/// Upstream Hugging Face repository for the official NVIDIA checkpoint.
pub const SOURCE_REPO_URL: &str = "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3";
/// Direct download URL for the official `.nemo` checkpoint.
pub const SOURCE_NEMO_URL: &str =
    "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3/resolve/main/parakeet-tdt-0.6b-v3.nemo";
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

/// Return the canonical cached Q8_0 model path.
///
/// # Returns
///
/// The full path to `parakeet-tdt-0.6b-v3-Q8_0.gguf` in the model cache.
///
/// # Errors
///
/// Returns an error if the platform cache directory cannot be determined.
pub fn cached_q8_model_path() -> Result<PathBuf> {
    Ok(models_dir()?.join(Q8_FILENAME))
}

/// Resolve the model path for daemon startup.
///
/// Explicit `-m/--model` paths always win. Without an explicit path, parakit
/// uses the canonical cached Q8_0 model produced by `parakit fetch`.
///
/// # Returns
///
/// The explicit model path, or the canonical cached Q8_0 path when no explicit
/// path was supplied.
///
/// # Errors
///
/// Returns an actionable error when no explicit model was supplied and the
/// canonical cached model does not exist.
pub fn resolve_model_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }

    let cached = cached_q8_model_path()?;
    if cached.is_file() {
        return Ok(cached);
    }

    Err(anyhow::anyhow!(
        "No cached model found. Run `parakit fetch` first. Expected: {}",
        cached.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_model_path_wins() {
        let path = Path::new("models/custom.gguf");
        assert_eq!(resolve_model_path(Some(path)).unwrap(), path);
    }

    #[test]
    fn canonical_filenames_are_q8_only() {
        assert_eq!(Q8_FILENAME, "parakeet-tdt-0.6b-v3-Q8_0.gguf");
        assert!(SOURCE_NEMO_URL.ends_with(NEMO_FILENAME));
    }
}
