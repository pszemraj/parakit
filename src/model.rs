//! Canonical Parakeet model names, cache paths, and model path helpers.

use anyhow::{Context, Result};
use directories::BaseDirs;
use std::path::{Path, PathBuf};

/// Upstream Hugging Face repository for the official NVIDIA checkpoint.
pub const OFFICIAL_NEMO_REPO_URL: &str = "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3";
/// Direct download URL for the official `.nemo` checkpoint.
pub const OFFICIAL_NEMO_URL: &str =
    "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3/resolve/main/parakeet-tdt-0.6b-v3.nemo";
/// Owner-hosted GGUF repository used by the default end-user fetch path.
pub const HOSTED_GGUF_REPO_URL: &str = "https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf";
/// Shared file stem for hosted GGUF artifacts.
pub const MODEL_STEM: &str = "parakeet-tdt-0.6b-v3";
/// Default hosted quantization loaded by parakit.
pub const DEFAULT_QUANT: &str = "Q8_0";
/// Hosted full-precision source GGUF for future local re-quantization.
pub const SOURCE_GGUF_QUANT: &str = "F16";
/// Direct download URL for the default hosted Q8_0 GGUF.
pub const HOSTED_Q8_URL: &str = "https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf/resolve/main/parakeet-tdt-0.6b-v3-Q8_0.gguf";
/// Expected SHA256 for the default hosted Q8_0 GGUF.
pub const HOSTED_Q8_SHA256: &str =
    "e8bc983c89342a1f36a5bfa1a7a2dc6fab8f9ebdc2e305738f36e3ff60cbc313";
/// File name for the downloaded official NeMo checkpoint.
pub const NEMO_FILENAME: &str = "parakeet-tdt-0.6b-v3.nemo";
/// File name for the intermediate F16 GGUF.
pub const F16_FILENAME: &str = "parakeet-tdt-0.6b-v3-F16.gguf";
/// File name for the hosted full-precision source GGUF.
pub const SOURCE_GGUF_FILENAME: &str = F16_FILENAME;
/// File name for the canonical Q8_0 GGUF used by parakit by default.
pub const Q8_FILENAME: &str = "parakeet-tdt-0.6b-v3-Q8_0.gguf";
/// File name for model acquisition metadata.
pub const MANIFEST_FILENAME: &str = "manifest.json";

/// Return the standard hosted GGUF file name for a quantization suffix.
///
/// # Returns
///
/// A file name in the form `parakeet-tdt-0.6b-v3-<QUANT>.gguf`.
pub fn hosted_gguf_filename(quant: &str) -> String {
    format!("{MODEL_STEM}-{quant}.gguf")
}

/// Return the direct Hugging Face download URL for a hosted GGUF file.
///
/// # Returns
///
/// The `/resolve/main/` URL for `filename` in the owner-hosted GGUF repo.
pub fn hosted_gguf_url(filename: &str) -> String {
    format!("{HOSTED_GGUF_REPO_URL}/resolve/main/{filename}")
}

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

/// Resolve a model path without downloading anything.
///
/// Explicit `-m/--model` paths always win. Without an explicit path, this
/// returns the canonical cached Q8_0 model if it exists.
///
/// # Returns
///
/// The explicit model path, or the canonical cached Q8_0 path when no explicit
/// path was supplied.
///
/// # Errors
///
/// Returns an actionable error when no explicit model was supplied and no
/// canonical cached model exists. Daemon startup normally uses
/// `fetch::ensure_default_model` instead so it can populate this cache.
pub fn resolve_model_path(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }

    let cached = cached_q8_model_path()?;
    if cached.is_file() {
        return Ok(cached);
    }

    Err(anyhow::anyhow!(
        "No cached model found at {}",
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
    fn hosted_model_constants_are_consistent() {
        assert_eq!(Q8_FILENAME, "parakeet-tdt-0.6b-v3-Q8_0.gguf");
        assert_eq!(SOURCE_GGUF_FILENAME, "parakeet-tdt-0.6b-v3-F16.gguf");
        assert_eq!(DEFAULT_QUANT, "Q8_0");
        assert_eq!(SOURCE_GGUF_QUANT, "F16");
        assert!(OFFICIAL_NEMO_URL.ends_with(NEMO_FILENAME));
        assert!(HOSTED_Q8_URL.ends_with(Q8_FILENAME));
        assert_eq!(HOSTED_Q8_SHA256.len(), 64);
    }

    #[test]
    fn hosted_gguf_names_follow_hub_convention() {
        assert_eq!(hosted_gguf_filename("F16"), SOURCE_GGUF_FILENAME);
        assert_eq!(hosted_gguf_filename("Q8_0"), Q8_FILENAME);
        assert_eq!(
            hosted_gguf_filename("Q5_K_M"),
            "parakeet-tdt-0.6b-v3-Q5_K_M.gguf"
        );
        assert_eq!(hosted_gguf_url(Q8_FILENAME), HOSTED_Q8_URL);
    }
}
