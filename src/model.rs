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
/// File name for the downloaded official NeMo checkpoint.
pub const NEMO_FILENAME: &str = "parakeet-tdt-0.6b-v3.nemo";
/// File name for the intermediate F16 GGUF.
pub const F16_FILENAME: &str = "parakeet-tdt-0.6b-v3-F16.gguf";
/// File name for the canonical Q8_0 GGUF used by parakit by default.
pub const Q8_FILENAME: &str = "parakeet-tdt-0.6b-v3-Q8_0.gguf";
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
/// The directory where model artifacts are stored.
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
/// Returns an error if `env_var` does not supply the base and `HOME` is unset or empty.
#[cfg(not(target_os = "windows"))]
pub fn xdg_base(env_var: &str, fallback_subdir: &str) -> Result<PathBuf> {
    resolve_xdg_base(
        std::env::var_os(env_var),
        std::env::var_os("HOME"),
        fallback_subdir,
    )
}

/// Resolve an XDG-style base directory from already-read environment values.
///
/// Keeping the environment reads in [`xdg_base`] lets tests cover empty and missing values without mutating process-global environment state.
/// An empty value counts as unset for both variables, so an empty `HOME` never yields a path relative to the working directory.
///
/// # Arguments
///
/// * `xdg` - Value of the XDG environment variable, if set.
/// * `home` - Value of `HOME`, if set.
/// * `fallback_subdir` - Subdirectory of `home` to fall back to.
///
/// # Returns
///
/// `xdg` when nonempty, otherwise `home` joined with `fallback_subdir`.
///
/// # Errors
///
/// Returns an error if `xdg` is unset or empty and `home` is unset or empty.
#[cfg(not(target_os = "windows"))]
fn resolve_xdg_base(
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    fallback_subdir: &str,
) -> Result<PathBuf> {
    if let Some(path) = xdg.filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    let home = home
        .filter(|home| !home.is_empty())
        .context("HOME is unset or empty")?;
    Ok(PathBuf::from(home).join(fallback_subdir))
}

/// Join a relative path's components with `/`, regardless of platform.
///
/// Shared core of `parakit cache list` display paths. The caller applies its own policy for
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

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn os(value: &str) -> Option<OsString> {
        Some(OsString::from(value))
    }

    #[test]
    fn nonempty_xdg_value_wins_over_home() {
        let base = resolve_xdg_base(os("/xdg/cache"), os("/home/user"), ".cache").unwrap();
        assert_eq!(base, PathBuf::from("/xdg/cache"));
    }

    #[test]
    fn unset_or_empty_xdg_value_falls_back_to_home() {
        for xdg in [None, os("")] {
            let base = resolve_xdg_base(xdg.clone(), os("/home/user"), ".cache").unwrap();
            assert_eq!(base, PathBuf::from("/home/user/.cache"), "{xdg:?}");
        }
    }

    #[test]
    fn unset_or_empty_home_is_an_error_not_a_relative_path() {
        for xdg in [None, os("")] {
            for home in [None, os("")] {
                let err = resolve_xdg_base(xdg.clone(), home.clone(), ".config")
                    .expect_err("no usable base must be an error");
                assert!(
                    err.to_string().contains("HOME is unset or empty"),
                    "xdg {xdg:?}, home {home:?}: {err:#}"
                );
            }
        }
    }
}
