//! Named model presets, model selection, and platform cache paths.

use anyhow::{Context, Result};
#[cfg(target_os = "windows")]
use directories::BaseDirs;
use std::path::{Path, PathBuf};

/// Weight format acquired for a named preset.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, clap::ValueEnum, serde::Deserialize)]
pub enum ModelQuantization {
    /// Use the provider's hosted Q8_0 artifact.
    #[default]
    #[value(name = "q8_0")]
    #[serde(rename = "q8_0")]
    Q8,
    /// Generate and cache Q6_K locally from the provider's F16 artifact.
    #[value(name = "q6_k")]
    #[serde(rename = "q6_k")]
    Q6K,
}

impl ModelQuantization {
    /// Return the spelling accepted by CLI and config.
    ///
    /// # Returns
    /// `q8_0` or `q6_k`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Q8 => "q8_0",
            Self::Q6K => "q6_k",
        }
    }
}

/// Hosted Q8_0 model choices supported by parakit.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    PartialEq,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum ModelPreset {
    /// NVIDIA's English-only Parakeet v2 model.
    #[value(name = "parakeet-v2")]
    #[serde(rename = "parakeet-v2")]
    ParakeetV2,
    /// The original NVIDIA Parakeet v3 model.
    #[value(name = "parakeet-v3")]
    #[serde(rename = "parakeet-v3")]
    ParakeetV3,
    /// Qwen3-ASR with 0.6 billion parameters.
    #[value(name = "qwen3-asr-0.6b")]
    #[serde(rename = "qwen3-asr-0.6b")]
    Qwen3Asr06b,
    /// Qwen3-ASR with 1.7 billion parameters.
    #[value(name = "qwen3-asr-1.7b")]
    #[serde(rename = "qwen3-asr-1.7b")]
    Qwen3Asr17b,
    /// Moondream's post-trained Parakeet v3 model and application default.
    #[default]
    #[value(name = "parakeet-ultra")]
    #[serde(rename = "parakeet-ultra")]
    ParakeetUltra,
    /// Experimental Redux for testing only; no packed ternary acceleration.
    #[value(name = "parakeet-redux")]
    #[serde(rename = "parakeet-redux")]
    ParakeetRedux,
}

impl ModelPreset {
    /// Return the spelling accepted by the CLI and configuration file.
    ///
    /// # Returns
    ///
    /// The preset's stable identifier.
    pub const fn id(self) -> &'static str {
        match self {
            Self::ParakeetV2 => "parakeet-v2",
            Self::ParakeetV3 => "parakeet-v3",
            Self::Qwen3Asr06b => "qwen3-asr-0.6b",
            Self::Qwen3Asr17b => "qwen3-asr-1.7b",
            Self::ParakeetUltra => "parakeet-ultra",
            Self::ParakeetRedux => "parakeet-redux",
        }
    }

    /// Return the Hugging Face repository that hosts this preset's GGUF.
    ///
    /// # Returns
    ///
    /// The hosted artifact's `owner/repo` identifier.
    pub const fn repo(self) -> &'static str {
        match self {
            Self::ParakeetV2 => "cstr/parakeet-tdt-0.6b-v2-GGUF",
            Self::ParakeetV3 => "pszemraj/parakeet-tdt-0.6b-v3-gguf",
            Self::Qwen3Asr06b => "cstr/qwen3-asr-0.6b-GGUF",
            Self::Qwen3Asr17b => "cstr/qwen3-asr-1.7b-GGUF",
            Self::ParakeetUltra => "cstr/parakeet-ultra-GGUF",
            Self::ParakeetRedux => "cstr/parakeet-redux-GGUF",
        }
    }

    /// Return the exact hosted Q8_0 filename; presets never auto-select a quant.
    ///
    /// # Returns
    ///
    /// The filename inside the hosted repository.
    pub const fn filename(self) -> &'static str {
        match self {
            Self::ParakeetV2 => "parakeet-tdt-0.6b-v2-q8_0.gguf",
            Self::ParakeetV3 => Q8_FILENAME,
            Self::Qwen3Asr06b => "qwen3-asr-0.6b-q8_0.gguf",
            Self::Qwen3Asr17b => "qwen3-asr-1.7b-q8_0.gguf",
            Self::ParakeetUltra => "parakeet-ultra-q8_0.gguf",
            Self::ParakeetRedux => "parakeet-redux-q8_0.gguf",
        }
    }

    /// Return the selected hosted artifact revision.
    ///
    /// # Returns
    /// A fixed provider commit for new presets, or the existing v3 `main`.
    pub const fn revision(self) -> &'static str {
        match self {
            Self::ParakeetV2 => "8878172f3c45ba231fac8cfd4aff2542b13bc875",
            Self::ParakeetV3 => "main",
            Self::Qwen3Asr06b => "58bd3202f835f46b24b17142cb503b0860c737a5",
            Self::Qwen3Asr17b => "674df5d44b50a63e7102a18895ed20e3f91de301",
            Self::ParakeetUltra => "252cd632a21e98ba5edbdeb61c7274d01c54cb73",
            Self::ParakeetRedux => "3f288b7fb33caff80f1424bd5ea1cecaf9808209",
        }
    }

    /// Return the original checkpoint revision used by source rebuilding.
    ///
    /// # Returns
    /// A fixed source commit for Qwen/Moondream; v3 uses its existing Nemo URL,
    /// and v2 has no source-rebuild path.
    pub const fn source_revision(self) -> Option<&'static str> {
        match self {
            Self::ParakeetV2 | Self::ParakeetV3 => None,
            Self::Qwen3Asr06b => Some("5eb144179a02acc5e5ba31e748d22b0cf3e303b0"),
            Self::Qwen3Asr17b => Some("7278e1e70fe206f11671096ffdd38061171dd6e5"),
            Self::ParakeetUltra => Some("73175eb7aeb0d82f1e2a6b53b3aabc10a90bcd0b"),
            Self::ParakeetRedux => Some("2bf128600aac4b16946f7ed8372e56117fe5e23b"),
        }
    }

    /// Return the original Hugging Face checkpoint used for source conversion.
    ///
    /// # Returns
    ///
    /// The checkpoint repository for Qwen and Moondream. V3 uses NVIDIA's
    /// `.nemo` workflow; v2 supports hosted GGUF acquisition only.
    pub const fn source_repo(self) -> Option<&'static str> {
        match self {
            Self::ParakeetV2 | Self::ParakeetV3 => None,
            Self::Qwen3Asr06b => Some("Qwen/Qwen3-ASR-0.6B"),
            Self::Qwen3Asr17b => Some("Qwen/Qwen3-ASR-1.7B"),
            Self::ParakeetUltra => Some("moondream/parakeet-ultra"),
            Self::ParakeetRedux => Some("moondream/parakeet-redux"),
        }
    }

    /// Return the CrispASR backend expected for this preset.
    ///
    /// # Returns
    ///
    /// `qwen3` for Qwen3-ASR or `parakeet` for the Parakeet models.
    pub const fn backend(self) -> &'static str {
        match self {
            Self::Qwen3Asr06b | Self::Qwen3Asr17b => "qwen3",
            Self::ParakeetV2 | Self::ParakeetV3 | Self::ParakeetUltra | Self::ParakeetRedux => {
                "parakeet"
            }
        }
    }
}

impl std::fmt::Display for ModelPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.id())
    }
}

/// A selected local GGUF or a named hosted model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelSelection {
    /// An explicit model file supplied by the user.
    Local(PathBuf),
    /// A hosted Q8_0 preset, downloaded when absent from the cache.
    Preset(ModelPreset),
}

/// Resolve CLI and configuration model fields without accessing the filesystem.
///
/// # Arguments
///
/// * `cli_model` - The CLI model path, if supplied.
/// * `cli_preset` - The CLI preset, if supplied.
/// * `config_model` - The configured model path, if supplied.
/// * `config_preset` - The configured preset, if supplied.
///
/// # Returns
///
/// The CLI selection, otherwise the configured selection, otherwise Parakeet Ultra.
///
/// # Errors
///
/// Returns an error when either source supplies both a model path and a preset,
/// including malformed configuration overridden by a valid CLI selection.
pub fn resolve_model_selection(
    cli_model: Option<&Path>,
    cli_preset: Option<ModelPreset>,
    config_model: Option<&Path>,
    config_preset: Option<ModelPreset>,
) -> Result<ModelSelection> {
    if cli_model.is_some() && cli_preset.is_some() {
        anyhow::bail!("--model and --model-preset cannot be used together");
    }
    if config_model.is_some() && config_preset.is_some() {
        anyhow::bail!("daemon.model and daemon.model_preset cannot be set together");
    }
    Ok(cli_model
        .map(|path| ModelSelection::Local(path.to_path_buf()))
        .or_else(|| cli_preset.map(ModelSelection::Preset))
        .or_else(|| config_model.map(|path| ModelSelection::Local(path.to_path_buf())))
        .or_else(|| config_preset.map(ModelSelection::Preset))
        .unwrap_or(ModelSelection::Preset(ModelPreset::default())))
}

/// Environment variable that overrides the platform model cache directory.
pub const MODELS_DIR_ENV: &str = "PARAKIT_MODELS_DIR";
/// Direct download URL for the official `.nemo` checkpoint.
pub const OFFICIAL_NEMO_URL: &str =
    "https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3/resolve/main/parakeet-tdt-0.6b-v3.nemo";
/// Direct download URL for the original Parakeet v3 hosted Q8_0 GGUF.
pub const HOSTED_Q8_URL: &str = "https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf/resolve/main/parakeet-tdt-0.6b-v3-Q8_0.gguf";
/// File name for the downloaded official NeMo checkpoint.
pub const NEMO_FILENAME: &str = "parakeet-tdt-0.6b-v3.nemo";
/// File name for the intermediate F16 GGUF.
pub const F16_FILENAME: &str = "parakeet-tdt-0.6b-v3-F16.gguf";
/// File name for the original Parakeet v3 Q8_0 GGUF, kept at the cache root.
pub const Q8_FILENAME: &str = "parakeet-tdt-0.6b-v3-Q8_0.gguf";
/// Environment variable overriding the Hugging Face Hub endpoint used by
/// `parakit fetch` for Hub API calls, repo downloads, preset downloads, and the
/// official-checkpoint URL. Useful for internal Nexus/Artifactory-style
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
    if let Some(path) = non_empty_env_path(MODELS_DIR_ENV, std::env::var_os(MODELS_DIR_ENV))? {
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

/// Resolve an optional explicit path override without reading the environment.
///
/// # Arguments
///
/// * `var_name` - Environment variable named in an empty-value error.
/// * `raw` - Already-read variable value; non-Unicode paths are preserved.
///
/// # Returns
///
/// The configured path, or `None` when the variable is unset.
///
/// # Errors
///
/// Returns an error when the variable is set to an empty value.
pub fn non_empty_env_path(
    var_name: &str,
    raw: Option<std::ffi::OsString>,
) -> Result<Option<PathBuf>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    if raw.is_empty() {
        anyhow::bail!("{var_name} is set but empty");
    }
    Ok(Some(PathBuf::from(raw)))
}

#[cfg(test)]
mod preset_tests {
    use super::*;
    use clap::ValueEnum;

    #[test]
    fn preset_names_and_artifacts_are_explicit() {
        for (preset, id, repo, filename, backend) in [
            (
                ModelPreset::ParakeetV2,
                "parakeet-v2",
                "cstr/parakeet-tdt-0.6b-v2-GGUF",
                "parakeet-tdt-0.6b-v2-q8_0.gguf",
                "parakeet",
            ),
            (
                ModelPreset::ParakeetV3,
                "parakeet-v3",
                "pszemraj/parakeet-tdt-0.6b-v3-gguf",
                "parakeet-tdt-0.6b-v3-Q8_0.gguf",
                "parakeet",
            ),
            (
                ModelPreset::Qwen3Asr06b,
                "qwen3-asr-0.6b",
                "cstr/qwen3-asr-0.6b-GGUF",
                "qwen3-asr-0.6b-q8_0.gguf",
                "qwen3",
            ),
            (
                ModelPreset::Qwen3Asr17b,
                "qwen3-asr-1.7b",
                "cstr/qwen3-asr-1.7b-GGUF",
                "qwen3-asr-1.7b-q8_0.gguf",
                "qwen3",
            ),
            (
                ModelPreset::ParakeetUltra,
                "parakeet-ultra",
                "cstr/parakeet-ultra-GGUF",
                "parakeet-ultra-q8_0.gguf",
                "parakeet",
            ),
            (
                ModelPreset::ParakeetRedux,
                "parakeet-redux",
                "cstr/parakeet-redux-GGUF",
                "parakeet-redux-q8_0.gguf",
                "parakeet",
            ),
        ] {
            assert_eq!(ModelPreset::from_str(id, false).unwrap(), preset);
            assert_eq!(
                serde_json::from_str::<ModelPreset>(&format!("\"{id}\"")).unwrap(),
                preset
            );
            assert_eq!(serde_json::to_string(&preset).unwrap(), format!("\"{id}\""));
            assert_eq!(preset.id(), id);
            assert_eq!(preset.to_string(), id);
            assert_eq!(preset.repo(), repo);
            assert_eq!(preset.filename(), filename);
            assert_eq!(preset.backend(), backend);
        }
        for unknown in ["qwen3", "parakeet", "Qwen3Asr06b", "qwen3-asr-0.6b-q4"] {
            assert!(ModelPreset::from_str(unknown, false).is_err());
            assert!(serde_json::from_str::<ModelPreset>(&format!("\"{unknown}\"")).is_err());
        }
    }

    #[test]
    fn selection_uses_cli_then_config_then_ultra_default() {
        let cli_path = Path::new("cli.gguf");
        let config_path = Path::new("config.gguf");
        assert_eq!(
            resolve_model_selection(None, None, None, None).unwrap(),
            ModelSelection::Preset(ModelPreset::ParakeetUltra)
        );
        assert_eq!(
            resolve_model_selection(None, None, Some(config_path), None).unwrap(),
            ModelSelection::Local(config_path.to_path_buf())
        );
        assert_eq!(
            resolve_model_selection(None, None, None, Some(ModelPreset::ParakeetUltra)).unwrap(),
            ModelSelection::Preset(ModelPreset::ParakeetUltra)
        );
        assert_eq!(
            resolve_model_selection(Some(cli_path), None, None, Some(ModelPreset::ParakeetUltra))
                .unwrap(),
            ModelSelection::Local(cli_path.to_path_buf())
        );
        assert_eq!(
            resolve_model_selection(
                None,
                Some(ModelPreset::Qwen3Asr17b),
                Some(config_path),
                None
            )
            .unwrap(),
            ModelSelection::Preset(ModelPreset::Qwen3Asr17b)
        );
        assert_eq!(
            resolve_model_selection(Some(cli_path), None, Some(config_path), None).unwrap(),
            ModelSelection::Local(cli_path.to_path_buf())
        );
    }

    #[test]
    fn same_source_conflicts_fail_even_when_cli_overrides_config() {
        let path = Path::new("model.gguf");
        assert!(
            resolve_model_selection(Some(path), Some(ModelPreset::ParakeetV3), None, None)
                .unwrap_err()
                .to_string()
                .contains("--model-preset")
        );
        for (cli_path, cli_preset) in [
            (None, None),
            (Some(path), None),
            (None, Some(ModelPreset::Qwen3Asr06b)),
        ] {
            assert!(resolve_model_selection(
                cli_path,
                cli_preset,
                Some(path),
                Some(ModelPreset::ParakeetUltra)
            )
            .unwrap_err()
            .to_string()
            .contains("daemon.model_preset"));
        }
    }
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
