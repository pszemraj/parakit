//! Canonical Parakeet model names and cache paths.

use anyhow::{Context, Result};
#[cfg(target_os = "windows")]
use directories::BaseDirs;
use std::path::{Path, PathBuf};

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
/// Environment variable overriding the Hugging Face Hub endpoint used by
/// `parakit fetch` for Hub API calls, repo downloads, and the two pinned
/// hosted/official-checkpoint URLs. Useful for internal Nexus/Artifactory-style
/// mirrors on networks where `huggingface.co` is blocked.
pub const HF_ENDPOINT_ENV: &str = "HF_ENDPOINT";
/// Default Hugging Face Hub endpoint used when `HF_ENDPOINT` is unset or blank.
pub const HF_DEFAULT_ENDPOINT: &str = "https://huggingface.co";
/// Environment variable supplying a bearer token for authenticated Hugging
/// Face Hub requests (gated repos or authenticated internal mirrors). Only
/// attached to requests that target the resolved Hub endpoint host, never to
/// an arbitrary `parakit fetch <url>` host.
pub const HF_TOKEN_ENV: &str = "HF_TOKEN";

/// Verify that a model path names a regular file.
///
/// This check is shared by daemon preflight, which must reject an explicit bad
/// path before initializing desktop/audio resources, and [`crate::inference`],
/// which must defend direct engine callers too.
///
/// # Returns
///
/// `Ok(())` when `path` is a regular file.
///
/// # Errors
///
/// Returns an error when `path` is not a regular file.
pub fn validate_model_file(path: &Path) -> Result<()> {
    if !path.is_file() {
        anyhow::bail!("model path is not a file: {}", path.display());
    }
    Ok(())
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
    if let Some(path) = override_models_dir()? {
        return Ok(path);
    }

    #[cfg(target_os = "windows")]
    {
        let dirs = BaseDirs::new().context("could not determine user cache directory")?;
        Ok(dirs
            .data_local_dir()
            .join("parakit")
            .join("Cache")
            .join("models"))
    }

    #[cfg(not(target_os = "windows"))]
    {
        Ok(xdg_cache_base()?.join("parakit").join("models"))
    }
}

/// Return the XDG-style cache base used by Unix-like parakit paths.
///
/// # Returns
///
/// `$XDG_CACHE_HOME` when set and non-empty, otherwise `$HOME/.cache`.
///
/// # Errors
///
/// Returns an error if no usable home directory is available.
#[cfg(not(target_os = "windows"))]
pub fn xdg_cache_base() -> Result<PathBuf> {
    xdg_base("XDG_CACHE_HOME", ".cache")
}

/// Resolve an XDG-style base directory: the named environment variable when
/// set and non-empty, otherwise `$HOME/<fallback_subdir>`.
///
/// Shared by [`xdg_cache_base`] and `config::xdg_config_base`, which are
/// otherwise identical apart from the variable name and fallback
/// subdirectory.
///
/// # Arguments
///
/// * `env_var` - XDG environment variable to check first (e.g.
///   `XDG_CACHE_HOME`).
/// * `fallback_subdir` - Subdirectory of `$HOME` to fall back to (e.g.
///   `.cache`).
///
/// # Returns
///
/// The resolved base directory.
///
/// # Errors
///
/// Returns an error if no usable home directory is available.
#[cfg(not(target_os = "windows"))]
pub fn xdg_base(env_var: &str, fallback_subdir: &str) -> Result<PathBuf> {
    if let Some(path) = std::env::var_os(env_var) {
        if !path.as_os_str().is_empty() {
            return Ok(PathBuf::from(path));
        }
    }

    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(fallback_subdir))
}

/// Join a relative path's components with `/`, regardless of platform.
///
/// Shared core of [`crate::fetch`]'s manifest key derivation and the
/// `parakit cache list` display path; each caller applies its own policy for
/// a path that turns out not to be relative to the expected base, so only
/// the join itself lives here.
///
/// # Arguments
///
/// * `rel` - A relative path, already stripped of its base directory.
///
/// # Returns
///
/// `rel`'s components joined with `/`.
pub fn slash_joined(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
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
