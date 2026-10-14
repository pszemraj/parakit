//! Original Hugging Face checkpoint conversion for the named model presets.

use super::{
    acquire_artifact_lock, ensure_q8, find_python, move_into_place, preset_cache_path,
    quantize_bin_path, remove_if_exists, run_command, FetchOptions,
};
use crate::model::{ModelPreset, HF_ENDPOINT_ENV, HF_TOKEN_ENV};
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Convert a preset's original checkpoint and quantize its F16 GGUF to Q8_0.
///
/// # Arguments
///
/// * `options` - Force, verbosity, and source settings for this transaction.
/// * `endpoint` - Resolved Hugging Face endpoint, including mirror overrides.
/// * `token` - Optional bearer token passed through the downloader's environment.
/// * `preset` - Named model whose original checkpoint is converted.
/// * `checkpoint_dir` - Optional user-owned snapshot, read without modification.
/// * `keep_f16` - Retain the intermediate F16 model after the Q8_0 model is ready.
/// * `models_dir` - Cache root for model artifacts and downloaded checkpoints.
///
/// # Returns
///
/// The same cached Q8_0 path used by the hosted preset workflow.
///
/// # Errors
///
/// Reports unsupported presets, missing tools or dependencies, invalid local
/// checkpoint paths, and download, conversion, quantization, or filesystem errors.
///
/// # Panics
///
/// Panics if the built-in preset cache path has no parent directory.
pub(super) fn run(
    options: &FetchOptions,
    endpoint: &str,
    token: Option<&str>,
    preset: ModelPreset,
    checkpoint_dir: Option<&Path>,
    keep_f16: bool,
    models_dir: &Path,
) -> Result<PathBuf> {
    let repo = preset
        .source_repo()
        .context("Parakeet v3 source conversion uses the existing NVIDIA .nemo workflow")?;
    let q8 = preset_cache_path(models_dir, preset);
    let dest_dir = q8.parent().expect("preset cache parent");
    std::fs::create_dir_all(dest_dir).with_context(|| format!("create {}", dest_dir.display()))?;
    let f16 = f16_path(&q8);
    let _lock = acquire_artifact_lock(&q8)?;
    if !options.force && q8.is_file() {
        options.verbose_status(format_args!(
            "parakit: using cached model: {}",
            q8.display()
        ));
        if !keep_f16 {
            remove_if_exists(&f16)?;
        }
        return Ok(q8);
    }

    let quantizer = quantize_bin_path()?;
    if options.force || !f16.is_file() {
        let revision = preset.source_revision().expect("source preset revision");
        let downloaded_dir = models_dir
            .join("checkpoints")
            .join(preset.id())
            .join(revision);
        let checkpoint = checkpoint_dir.unwrap_or(&downloaded_dir);
        if checkpoint_dir.is_some() && !checkpoint.is_dir() {
            bail!(
                "checkpoint path is not a directory: {}",
                checkpoint.display()
            );
        }
        let converter = converter_path(preset);
        if !converter.is_file() {
            bail!(
                "converter script not found at {}. Run from a full parakit checkout with the CrispASR submodule initialized.",
                converter.display()
            );
        }
        let python = conversion_python(preset, checkpoint_dir.is_none())?;
        if checkpoint_dir.is_none() {
            let downloader =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/download_asr_checkpoint.py");
            options.status(format_args!("parakit: downloading checkpoint {repo}"));
            let mut command = Command::new(&python);
            command
                .arg(downloader)
                .arg(repo)
                .arg(checkpoint)
                .arg("--revision")
                .arg(revision)
                .env(HF_ENDPOINT_ENV, endpoint)
                .env(HF_TOKEN_ENV, token.unwrap_or(""));
            if options.force {
                command.arg("--force");
            }
            if options.quiet {
                command
                    .stdout(Stdio::null())
                    .env("HF_HUB_DISABLE_PROGRESS_BARS", "1");
            }
            run_command(&mut command, "download original Hugging Face checkpoint")?;
        }
        convert_f16(preset, checkpoint, &f16, &python, &converter, options)?;
    } else {
        options.status(format_args!(
            "parakit: using cached F16 GGUF: {}",
            f16.display()
        ));
    }
    ensure_q8(&f16, &q8, &quantizer, options)?;
    if !keep_f16 {
        remove_if_exists(&f16)?;
    }
    options.status(format_args!("parakit: model ready: {}", q8.display()));
    Ok(q8)
}

fn f16_path(q8: &Path) -> PathBuf {
    let filename = q8.file_name().expect("preset filename").to_string_lossy();
    q8.with_file_name(filename.replace("-q8_0.gguf", "-f16.gguf"))
}

fn converter_path(preset: ModelPreset) -> PathBuf {
    let filename = if preset.backend() == "qwen3" {
        "convert-qwen3-asr-to-gguf.py"
    } else {
        "convert-parakeet-to-gguf.py"
    };
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("vendor/CrispASR/models")
        .join(filename)
}

fn conversion_python(preset: ModelPreset, download: bool) -> Result<PathBuf> {
    let python = find_python()?;
    let mut imports = "import gguf, numpy, torch, safetensors".to_string();
    if preset.backend() == "parakeet" {
        imports.push_str(", sentencepiece, yaml, librosa");
    }
    if download {
        imports.push_str(", huggingface_hub");
    }
    let status = Command::new(&python)
        .arg("-c")
        .arg(imports)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("run {}", python.display()))?;
    if !status.success() {
        bail!(
            "Checkpoint conversion requires Python dependencies. Install with: pip install -r scripts/requirements-convert.txt"
        );
    }
    Ok(python)
}

fn convert_f16(
    preset: ModelPreset,
    checkpoint: &Path,
    f16: &Path,
    python: &Path,
    converter: &Path,
    options: &FetchOptions,
) -> Result<()> {
    let staging = f16.with_extension("gguf.converting");
    remove_if_exists(&staging)?;
    options.status(format_args!("parakit: converting checkpoint to F16 GGUF"));
    let input_flag = if preset.backend() == "qwen3" {
        "--input"
    } else {
        "--hf"
    };
    let mut command = Command::new(python);
    command
        .arg(converter)
        .arg(input_flag)
        .arg(checkpoint)
        .arg("--output")
        .arg(&staging);
    if options.quiet {
        command.stdout(Stdio::null());
    }
    if let Err(error) = run_command(&mut command, "convert checkpoint to F16 GGUF") {
        let _ = remove_if_exists(&staging);
        return Err(error);
    }
    move_into_place(&staging, f16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::FetchSource;

    fn options(preset: ModelPreset, force: bool) -> FetchOptions {
        FetchOptions {
            force,
            quiet: true,
            verbose: false,
            source: FetchSource::PresetSource {
                preset,
                checkpoint_dir: None,
                keep_f16: false,
            },
        }
    }

    #[test]
    fn source_presets_map_to_original_checkpoints_and_family_converters() {
        for (preset, repo, converter) in [
            (
                ModelPreset::Qwen3Asr06b,
                "Qwen/Qwen3-ASR-0.6B",
                "convert-qwen3-asr-to-gguf.py",
            ),
            (
                ModelPreset::Qwen3Asr17b,
                "Qwen/Qwen3-ASR-1.7B",
                "convert-qwen3-asr-to-gguf.py",
            ),
            (
                ModelPreset::ParakeetUltra,
                "moondream/parakeet-ultra",
                "convert-parakeet-to-gguf.py",
            ),
            (
                ModelPreset::ParakeetRedux,
                "moondream/parakeet-redux",
                "convert-parakeet-to-gguf.py",
            ),
        ] {
            assert_eq!(preset.source_repo(), Some(repo));
            assert!(converter_path(preset).ends_with(converter));
            let q8 = preset_cache_path(Path::new("models"), preset);
            assert_eq!(
                f16_path(&q8).file_name().unwrap(),
                preset
                    .filename()
                    .replace("-q8_0.gguf", "-f16.gguf")
                    .as_str()
            );
        }
        assert_eq!(ModelPreset::ParakeetV3.source_repo(), None);
    }

    #[test]
    fn cached_source_models_skip_tools_network_and_supplied_checkpoint_checks() {
        let root = crate::test_support::fixture_root("parakit-fetch-tests", "source-presets-cache");
        for preset in [
            ModelPreset::Qwen3Asr06b,
            ModelPreset::Qwen3Asr17b,
            ModelPreset::ParakeetUltra,
            ModelPreset::ParakeetRedux,
        ] {
            let q8 = preset_cache_path(&root, preset);
            std::fs::create_dir_all(q8.parent().unwrap()).unwrap();
            std::fs::write(&q8, b"existing Q8").unwrap();
            let f16 = f16_path(&q8);
            std::fs::write(&f16, b"existing F16").unwrap();
            for keep_f16 in [true, false] {
                assert_eq!(
                    run(
                        &options(preset, false),
                        "https://example.invalid",
                        None,
                        preset,
                        Some(Path::new("missing-checkpoint")),
                        keep_f16,
                        &root
                    )
                    .unwrap(),
                    q8
                );
                assert_eq!(f16.exists(), keep_f16);
            }
            assert_eq!(std::fs::read(&q8).unwrap(), b"existing Q8");
        }
    }

    #[cfg(unix)]
    #[test]
    fn failed_conversion_and_quantization_clean_staging_and_preserve_existing_artifacts() {
        use std::os::unix::fs::PermissionsExt;
        let root =
            crate::test_support::fixture_root("parakit-fetch-tests", "source-presets-failure");
        let q8 = root.join("existing-q8.gguf");
        let f16 = root.join("existing-f16.gguf");
        std::fs::write(&q8, b"existing Q8").unwrap();
        std::fs::write(&f16, b"existing F16").unwrap();
        let fake_python = root.join("fake-python");
        std::fs::write(&fake_python, "#!/bin/sh\nprintf partial > \"$5\"\nexit 1\n").unwrap();
        std::fs::set_permissions(&fake_python, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(convert_f16(
            ModelPreset::Qwen3Asr06b,
            &root,
            &f16,
            &fake_python,
            Path::new("converter.py"),
            &options(ModelPreset::Qwen3Asr06b, true)
        )
        .is_err());
        assert!(!f16.with_extension("gguf.converting").exists());
        assert_eq!(std::fs::read(&f16).unwrap(), b"existing F16");
        let quantizer = root.join("fake-quantizer");
        std::fs::write(&quantizer, "#!/bin/sh\nprintf partial > \"$2\"\nexit 1\n").unwrap();
        std::fs::set_permissions(&quantizer, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(ensure_q8(
            &f16,
            &q8,
            &quantizer,
            &options(ModelPreset::Qwen3Asr06b, true)
        )
        .is_err());
        assert!(!q8.with_extension("gguf.quantizing").exists());
        assert_eq!(std::fs::read(&q8).unwrap(), b"existing Q8");
    }
}
