//! Model acquisition manifest persistence.
//!
//! `manifest.json` lives beside the cached models in [`crate::model::models_dir`]
//! and records enough provenance to decide whether a cached artifact is still
//! current: the single-model hosted/source-build fields kept by earlier
//! releases, plus a `downloads` map covering every extra model fetched via a
//! Hugging Face repo or direct URL `parakit fetch` source.

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use crate::model::HOSTED_Q8_SHA256;

const ACQ_HOSTED_Q8: &str = "hosted-q8";
/// `Manifest::acquisition` tag for a source rebuild from NVIDIA's official
/// `.nemo` checkpoint (`parakit fetch --from-source`).
pub(super) const ACQ_OFFICIAL_NEMO: &str = "official-nemo";

/// Record of one `parakit fetch` download into a `hub/` or `url/` cache
/// subdirectory, keyed by its path relative to `models_dir()` in
/// [`Manifest::downloads`].
#[derive(Debug, Clone, Deserialize, Serialize)]
pub(super) struct DownloadRecord {
    /// The exact URL last downloaded (resolve URL for a Hub repo file, or
    /// the user-supplied URL for a direct fetch).
    pub(super) source_url: String,
    /// SHA256 used to verify the download: `--sha256` override, Hub-reported
    /// LFS checksum, or `None` when no checksum was available at all.
    pub(super) sha256: Option<String>,
    /// RFC 3339 timestamp of the fetch.
    pub(super) fetched_at: String,
}

/// Model acquisition metadata persisted as `manifest.json`.
///
/// `#[serde(default)]` keeps older manifests without newer fields (including
/// `downloads`, added for multi-source fetch) parsing successfully instead of
/// failing to load.
#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct Manifest {
    pub(super) acquisition: String,
    pub(super) source_url: String,
    pub(super) nemo_sha256: Option<String>,
    pub(super) f16_input_sha256: Option<String>,
    pub(super) f16_sha256: Option<String>,
    pub(super) q8_input_sha256: Option<String>,
    pub(super) q8_sha256: Option<String>,
    pub(super) q8_output_path: String,
    pub(super) converter_script: String,
    pub(super) converter_crispasr_git_sha: String,
    pub(super) crispasr_quantize_bin: String,
    pub(super) crispasr_quantize_version: String,
    pub(super) downloaded_at: Option<String>,
    pub(super) converted_at: Option<String>,
    pub(super) quantized_at: Option<String>,
    /// Extra models fetched via a Hugging Face repo or direct URL source,
    /// keyed by their path relative to `models_dir()` using `/` separators
    /// (e.g. `"hub/cstr--parakeet-tdt-0.6b-v3-GGUF/parakeet-tdt-0.6b-v3-q8_0.gguf"`
    /// or `"url/model.gguf"`), independent of the single-model fields above.
    #[serde(default)]
    pub(super) downloads: BTreeMap<String, DownloadRecord>,
}

impl Manifest {
    /// Load a manifest from `path`, if it exists.
    ///
    /// # Returns
    ///
    /// `None` when `path` does not exist, else the parsed manifest.
    ///
    /// # Errors
    ///
    /// Returns an error if `path` exists but cannot be opened or parsed.
    pub(super) fn load(path: &Path) -> Result<Option<Self>> {
        if !path.is_file() {
            return Ok(None);
        }
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let manifest =
            serde_json::from_reader(file).with_context(|| format!("parse {}", path.display()))?;
        Ok(Some(manifest))
    }

    /// Write the manifest to `path` as pretty-printed JSON.
    ///
    /// # Returns
    ///
    /// `Ok(())` once `path` has been written.
    ///
    /// # Errors
    ///
    /// Returns an error if `path` cannot be created or written.
    pub(super) fn save(&self, path: &Path) -> Result<()> {
        let mut file = File::create(path).with_context(|| format!("create {}", path.display()))?;
        serde_json::to_writer_pretty(&mut file, self)?;
        file.write_all(b"\n")?;
        file.flush()?;
        Ok(())
    }

    /// Whether the recorded default-model acquisition matches `source_url`
    /// (the pinned URL actually used this run, after any `HF_ENDPOINT`
    /// rewrite) and `q8_path`.
    ///
    /// # Arguments
    ///
    /// * `q8_path` - The canonical cached Q8_0 model path.
    /// * `source_url` - The hosted Q8_0 URL actually used this run.
    ///
    /// # Returns
    ///
    /// `true` when the manifest already records this exact acquisition.
    pub(super) fn hosted_current(&self, q8_path: &Path, source_url: &str) -> bool {
        self.acquisition == ACQ_HOSTED_Q8
            && self.source_url == source_url
            && self.q8_sha256.as_deref() == Some(HOSTED_Q8_SHA256)
            && self.q8_output_path == q8_path.display().to_string()
    }

    /// Record a fresh hosted Q8_0 acquisition, clearing any stale
    /// source-build fields.
    ///
    /// # Arguments
    ///
    /// * `q8_path` - The canonical cached Q8_0 model path.
    /// * `source_url` - The hosted Q8_0 URL actually used this run.
    pub(super) fn mark_hosted_ready(&mut self, q8_path: &Path, source_url: &str) {
        self.acquisition = ACQ_HOSTED_Q8.to_string();
        self.source_url = source_url.to_string();
        self.q8_sha256 = Some(HOSTED_Q8_SHA256.to_string());
        self.q8_output_path = q8_path.display().to_string();
        self.downloaded_at = Some(now_utc());
        self.nemo_sha256 = None;
        self.f16_input_sha256 = None;
        self.f16_sha256 = None;
        self.q8_input_sha256 = None;
        self.converter_script.clear();
        self.converter_crispasr_git_sha.clear();
        self.crispasr_quantize_bin.clear();
        self.crispasr_quantize_version.clear();
        self.converted_at = None;
        self.quantized_at = None;
    }

    /// Whether the recorded source-build acquisition matches `source_url`
    /// (the pinned NeMo URL actually used this run, after any `HF_ENDPOINT`
    /// rewrite) and every converter/quantizer identity input.
    ///
    /// # Arguments
    ///
    /// * `q8_path` - The canonical cached Q8_0 model path.
    /// * `converter_script` - Path to the `.nemo`-to-GGUF converter script.
    /// * `crispasr_sha` - Git SHA of the vendored CrispASR checkout.
    /// * `quantize_bin` - Path to the `crispasr-quantize` binary.
    /// * `quantize_version` - Identity string for `quantize_bin` (see
    ///   `super::quantize_version`).
    /// * `source_url` - The official `.nemo` URL actually used this run.
    ///
    /// # Returns
    ///
    /// `true` when the manifest already records this exact source build.
    ///
    /// # Errors
    ///
    /// Returns an error if `q8_path` cannot be hashed.
    pub(super) fn final_current(
        &self,
        q8_path: &Path,
        converter_script: &Path,
        crispasr_sha: &str,
        quantize_bin: &Path,
        quantize_version: &str,
        source_url: &str,
    ) -> Result<bool> {
        if self.acquisition != ACQ_OFFICIAL_NEMO
            || self.source_url != source_url
            || !q8_path.is_file()
        {
            return Ok(false);
        }
        if self.converter_script != converter_script.display().to_string()
            || self.converter_crispasr_git_sha != crispasr_sha
            || self.crispasr_quantize_bin != quantize_bin.display().to_string()
            || self.crispasr_quantize_version != quantize_version
        {
            return Ok(false);
        }
        let Some(recorded_q8) = &self.q8_sha256 else {
            return Ok(false);
        };
        Ok(crate::checksum::sha256_file_hex(q8_path)? == *recorded_q8)
    }

    /// Record (or replace) the download entry for `relpath`.
    ///
    /// # Arguments
    ///
    /// * `relpath` - Manifest key: `path` relative to `models_dir`, joined
    ///   with `/` (see [`relative_key`]).
    /// * `source_url` - The exact URL downloaded.
    /// * `sha256` - The checksum used to verify the download, if any.
    pub(super) fn record_download(
        &mut self,
        relpath: &str,
        source_url: &str,
        sha256: Option<String>,
    ) {
        self.downloads.insert(
            relpath.to_string(),
            DownloadRecord {
                source_url: source_url.to_string(),
                sha256,
                fetched_at: now_utc(),
            },
        );
    }

    /// Return the recorded SHA256 for `relpath`, if any.
    ///
    /// # Returns
    ///
    /// The recorded checksum, or `None` when `relpath` has no entry or was
    /// recorded without one.
    pub(super) fn recorded_sha256(&self, relpath: &str) -> Option<String> {
        self.downloads
            .get(relpath)
            .and_then(|record| record.sha256.clone())
    }
}

/// Compute the `downloads` map key for `path`: its components relative to
/// `models_dir`, joined with `/` regardless of platform so the manifest
/// stays portable and diffable.
///
/// # Arguments
///
/// * `models_dir` - The model cache directory, as returned by
///   [`crate::model::models_dir`].
/// * `path` - Absolute path to a file under `models_dir`.
///
/// # Returns
///
/// `path`'s components relative to `models_dir`, joined with `/`.
///
/// # Errors
///
/// Returns an error if `path` is not under `models_dir`.
pub(super) fn relative_key(models_dir: &Path, path: &Path) -> Result<String> {
    let rel = path.strip_prefix(models_dir).with_context(|| {
        format!(
            "{} is not under model cache dir {}",
            path.display(),
            models_dir.display()
        )
    })?;
    Ok(crate::model::slash_joined(rel))
}

/// Current UTC time as an RFC 3339 timestamp, millisecond precision.
///
/// # Returns
///
/// The current time formatted like `2026-07-30T12:00:00.000Z`.
pub(super) fn now_utc() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::HOSTED_Q8_URL;

    #[test]
    fn hosted_manifest_records_default_model() {
        let mut manifest = Manifest::default();
        let path = Path::new("target/tmp/parakit-fetch-tests/model.gguf");
        manifest.mark_hosted_ready(path, HOSTED_Q8_URL);

        assert!(manifest.hosted_current(path, HOSTED_Q8_URL));
        assert_eq!(manifest.acquisition, ACQ_HOSTED_Q8);
        assert_eq!(manifest.source_url, HOSTED_Q8_URL);
        assert_eq!(manifest.q8_sha256.as_deref(), Some(HOSTED_Q8_SHA256));
        assert!(manifest.nemo_sha256.is_none());
        assert!(manifest.f16_sha256.is_none());
    }

    #[test]
    fn hosted_current_rejects_a_different_resolved_url() {
        // A cache populated while pointed at a mirror must not be treated as
        // current once `HF_ENDPOINT` reverts (or vice versa): the recorded
        // URL is the one actually used, not the compiled-in constant.
        let mut manifest = Manifest::default();
        let path = Path::new("target/tmp/parakit-fetch-tests/model2.gguf");
        manifest.mark_hosted_ready(path, "https://internal-mirror.example.com/foo.gguf");

        assert!(!manifest.hosted_current(path, HOSTED_Q8_URL));
        assert!(manifest.hosted_current(path, "https://internal-mirror.example.com/foo.gguf"));
    }

    #[test]
    fn download_round_trips_through_json() {
        let dir = crate::test_support::fixture_root("parakit-fetch-tests", "manifest-downloads");
        let path = dir.join("manifest.json");

        let mut manifest = Manifest::default();
        manifest.record_download(
            "hub/cstr--parakeet-tdt-0.6b-v3-GGUF/parakeet-tdt-0.6b-v3-q8_0.gguf",
            "https://huggingface.co/cstr/parakeet-tdt-0.6b-v3-GGUF/resolve/main/parakeet-tdt-0.6b-v3-q8_0.gguf",
            Some("a".repeat(64)),
        );
        manifest.record_download("url/model.gguf", "https://example.com/model.gguf", None);
        manifest.save(&path).unwrap();

        let loaded = Manifest::load(&path)
            .unwrap()
            .expect("manifest should load");
        assert_eq!(
            loaded.recorded_sha256(
                "hub/cstr--parakeet-tdt-0.6b-v3-GGUF/parakeet-tdt-0.6b-v3-q8_0.gguf"
            ),
            Some("a".repeat(64))
        );
        assert_eq!(loaded.recorded_sha256("url/model.gguf"), None);
        assert_eq!(loaded.recorded_sha256("hub/nope/nope.gguf"), None);
        assert_eq!(loaded.downloads.len(), 2);
    }

    #[test]
    fn old_manifest_without_downloads_field_still_parses() {
        // Manifests written before multi-source fetch have none of the
        // `downloads` bookkeeping. `#[serde(default)]` must keep them loading.
        let dir = crate::test_support::fixture_root("parakit-fetch-tests", "manifest-old");
        let path = dir.join("manifest.json");
        std::fs::write(
            &path,
            r#"{
  "acquisition": "hosted-q8",
  "source_url": "https://huggingface.co/pszemraj/parakeet-tdt-0.6b-v3-gguf/resolve/main/parakeet-tdt-0.6b-v3-Q8_0.gguf",
  "q8_sha256": "abc",
  "q8_output_path": "/tmp/model.gguf"
}
"#,
        )
        .unwrap();

        let manifest = Manifest::load(&path)
            .unwrap()
            .expect("old manifest should load");
        assert_eq!(manifest.acquisition, ACQ_HOSTED_Q8);
        assert!(manifest.downloads.is_empty());
        assert_eq!(manifest.recorded_sha256("hub/anything"), None);
    }

    #[test]
    fn relative_key_uses_forward_slashes() {
        let models_dir = Path::new("/cache/parakit/models");
        let path = models_dir
            .join("hub")
            .join("cstr--parakeet-tdt-0.6b-v3-GGUF")
            .join("parakeet-tdt-0.6b-v3-q8_0.gguf");

        assert_eq!(
            relative_key(models_dir, &path).unwrap(),
            "hub/cstr--parakeet-tdt-0.6b-v3-GGUF/parakeet-tdt-0.6b-v3-q8_0.gguf"
        );
    }

    #[test]
    fn relative_key_rejects_a_path_outside_models_dir() {
        let models_dir = Path::new("/cache/parakit/models");
        let path = Path::new("/elsewhere/model.gguf");
        assert!(relative_key(models_dir, path).is_err());
    }
}
