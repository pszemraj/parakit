//! Model acquisition for `parakit fetch`: hosted Q8_0 presets, a
//! source rebuild from original checkpoints, a Hugging Face repo (see [`hub`]),
//! or a direct URL.

mod hub;
mod net;
mod output;
mod source;

pub use hub::source_from_cli;

use crate::model::{
    models_dir, ModelPreset, ModelQuantization, F16_FILENAME, NEMO_FILENAME, OFFICIAL_NEMO_URL,
    Q8_FILENAME,
};
use anyhow::{anyhow, bail, Context, Result};
use fs2::FileExt;
use reqwest::blocking::Client;
use sha2::{Digest, Sha256};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Options for `parakit fetch`.
#[derive(Clone, Debug)]
pub struct FetchOptions {
    /// Ignore existing cache entries and rebuild all artifacts.
    pub force: bool,
    /// Suppress stdout status messages.
    pub quiet: bool,
    /// Print cache-hit diagnostics that are otherwise hidden.
    pub verbose: bool,
    /// Which acquisition path to use.
    pub source: FetchSource,
}

/// Model acquisition source for `parakit fetch`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FetchSource {
    /// Download the owner-hosted Q8_0 GGUF.
    HostedQ8,
    /// Download the exact hosted Q8_0 artifact for a named model preset.
    Preset(ModelPreset),
    /// Download a preset's F16 GGUF and cache a locally quantized Q6_K.
    PresetQ6(ModelPreset),
    /// Convert an original Hugging Face checkpoint and quantize it to Q8_0.
    PresetSource {
        /// Named model whose original checkpoint is converted.
        preset: ModelPreset,
        /// User-owned local snapshot; absent downloads into the model cache.
        checkpoint_dir: Option<PathBuf>,
        /// Keep the intermediate F16 GGUF after Q8_0 is ready.
        keep_f16: bool,
    },
    /// Rebuild Q8_0 locally from NVIDIA's official `.nemo` checkpoint.
    OfficialNemo {
        /// Keep the downloaded `.nemo` after the final Q8_0 model is produced.
        keep_nemo: bool,
        /// Keep the intermediate F16 GGUF after the final Q8_0 model is produced.
        keep_f16: bool,
    },
    /// Download a `.gguf` file out of a Hugging Face repo.
    HubRepo {
        /// `owner/repo`, without a `@revision` suffix.
        repo: String,
        /// Revision to fetch from. `None` defers to the Hub default (`main`).
        revision: Option<String>,
        /// Exact `rfilename` to select. `None` triggers automatic selection.
        file: Option<String>,
        /// User-supplied expected SHA256.
        sha256: Option<String>,
    },
    /// Download an arbitrary file by direct URL.
    Url {
        /// The URL to download.
        url: String,
        /// Expected SHA256; the only verification available for a URL source.
        sha256: Option<String>,
    },
}

/// Ensure the default hosted Q8_0 model is present in the cache.
///
/// # Arguments
///
/// * `quiet` - Suppress stdout status messages.
/// * `verbose` - Print cache-hit diagnostics when an existing model is used.
///
/// # Returns
///
/// The canonical cached Q8_0 model path.
///
/// # Errors
///
/// Returns an error if the model cannot be downloaded, verified, or written
/// into the platform cache directory.
pub fn ensure_default_model_with_verbosity(quiet: bool, verbose: bool) -> Result<PathBuf> {
    ensure_preset_model_with_verbosity(
        ModelPreset::ParakeetV3,
        ModelQuantization::Q8,
        quiet,
        verbose,
    )
}

/// Ensure a preset's selected precision is present in the cache.
///
/// # Arguments
///
/// * `preset` - The named hosted model to use.
/// * `quantization` - Hosted Q8_0 or locally generated Q6_K.
/// * `quiet` - Suppress stdout status messages.
/// * `verbose` - Print cache-hit diagnostics when an existing model is used.
///
/// # Returns
///
/// The preset's cached GGUF path, without any network request on a cache hit.
///
/// # Errors
///
/// Returns an error if the model cannot be downloaded or written to the cache.
pub fn ensure_preset_model_with_verbosity(
    preset: ModelPreset,
    quantization: ModelQuantization,
    quiet: bool,
    verbose: bool,
) -> Result<PathBuf> {
    acquire(&FetchOptions {
        force: false,
        quiet,
        verbose,
        source: match quantization {
            ModelQuantization::Q8 => FetchSource::Preset(preset),
            ModelQuantization::Q6K => FetchSource::PresetQ6(preset),
        },
    })
}

/// Run a model acquisition pipeline.
///
/// # Returns
///
/// The path to the model the requested [`FetchSource`] produced: the
/// preset cache path for [`FetchSource::Preset`] and [`FetchSource::PresetSource`], canonical cached Q8_0 path for
/// [`FetchSource::HostedQ8`] and [`FetchSource::OfficialNemo`], or the `hub/…`/`url/…` cache path for
/// [`FetchSource::HubRepo`]/[`FetchSource::Url`]. [`FetchSource::PresetQ6`] returns
/// a separate cached Q6_K path beside the preset's hosted F16 source.
///
/// # Errors
///
/// Returns an error if the model cannot be looked up, downloaded, verified,
/// converted, quantized, or written into the platform cache directory.
pub fn run(options: FetchOptions) -> Result<PathBuf> {
    let path = acquire(&options)?;
    options.ready(&path);
    Ok(path)
}

fn acquire(options: &FetchOptions) -> Result<PathBuf> {
    let endpoint = net::resolve_endpoint();
    let token = net::resolve_token();
    match &options.source {
        FetchSource::HostedQ8 => run_preset(
            options,
            &endpoint,
            token.as_deref(),
            ModelPreset::ParakeetV3,
            &models_dir()?,
        ),
        FetchSource::Preset(preset) => run_preset(
            options,
            &endpoint,
            token.as_deref(),
            *preset,
            &models_dir()?,
        ),
        FetchSource::PresetQ6(preset) => run_preset_q6(
            options,
            &endpoint,
            token.as_deref(),
            *preset,
            &models_dir()?,
        ),
        FetchSource::PresetSource {
            preset,
            checkpoint_dir,
            keep_f16,
        } => source::run(
            options,
            &endpoint,
            token.as_deref(),
            *preset,
            checkpoint_dir.as_deref(),
            *keep_f16,
            &models_dir()?,
        ),
        FetchSource::OfficialNemo {
            keep_nemo,
            keep_f16,
        } => run_official_nemo(options, &endpoint, token.as_deref(), *keep_nemo, *keep_f16),
        FetchSource::HubRepo {
            repo,
            revision,
            file,
            sha256,
        } => hub::run_hub_repo(
            options,
            &endpoint,
            token.as_deref(),
            repo,
            revision.as_deref(),
            file.as_deref(),
            sha256.as_deref(),
        ),
        FetchSource::Url { url, sha256 } => run_url(options, url, sha256.as_deref()),
    }
}

fn preset_cache_path(models_dir: &Path, preset: ModelPreset) -> PathBuf {
    if preset == ModelPreset::ParakeetV3 {
        return models_dir.join(Q8_FILENAME);
    }
    let (owner, repo) = preset.repo().split_once('/').expect("preset owner/repo");
    hub::hub_cache_dir(
        models_dir,
        owner,
        repo,
        preset.revision(),
        preset.filename(),
    )
    .join(preset.filename())
}

fn run_preset(
    options: &FetchOptions,
    endpoint: &str,
    token: Option<&str>,
    preset: ModelPreset,
    models_dir: &Path,
) -> Result<PathBuf> {
    let dest = preset_cache_path(models_dir, preset);
    let dest_dir = dest.parent().expect("preset cache parent");
    std::fs::create_dir_all(dest_dir).with_context(|| format!("create {}", dest_dir.display()))?;

    if !options.force && use_cached_download(options, &dest, None)? {
        return Ok(dest);
    }

    let (owner, repo) = preset.repo().split_once('/').expect("preset owner/repo");
    let url = hub::resolve_url(endpoint, owner, repo, preset.revision(), preset.filename());
    let client = net::build_client()?;
    let bearer = net::bearer_for(&url, endpoint, token);
    options.downloading(&url);
    download_and_verify(options, &client, &url, &dest, bearer, None)?;

    Ok(dest)
}

fn run_preset_q6(
    options: &FetchOptions,
    endpoint: &str,
    token: Option<&str>,
    preset: ModelPreset,
    models_dir: &Path,
) -> Result<PathBuf> {
    let f16_filename = match preset {
        ModelPreset::ParakeetV2 => "parakeet-tdt-0.6b-v2.gguf",
        ModelPreset::ParakeetV3 => F16_FILENAME,
        ModelPreset::Qwen3Asr06b => "qwen3-asr-0.6b.gguf",
        ModelPreset::Qwen3Asr17b => "qwen3-asr-1.7b-f16.gguf",
        ModelPreset::ParakeetUltra => "parakeet-ultra-f16.gguf",
        ModelPreset::ParakeetRedux => "parakeet-redux-f16.gguf",
    };
    let (owner, repo) = preset.repo().split_once('/').expect("preset owner/repo");
    let dir = hub::hub_cache_dir(models_dir, owner, repo, preset.revision(), f16_filename);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let q6 = dir.join(
        preset
            .filename()
            .to_ascii_lowercase()
            .replace("-q8_0.gguf", "-q6_k.gguf"),
    );
    let _lock = acquire_artifact_lock(&q6)?;
    if !options.force && q6.is_file() {
        return Ok(q6);
    }
    options.status(format_args!(
        "parakit: preparing {preset} Q6_K; a larger temporary download may be needed"
    ));
    let quantizer = quantize_bin_path()?;
    let f16 = dir.join(f16_filename);
    if options.force || !use_cached_download(options, &f16, None)? {
        let url = hub::resolve_url(endpoint, owner, repo, preset.revision(), f16_filename);
        let client = net::build_client()?;
        options.downloading(&url);
        download_and_verify(
            options,
            &client,
            &url,
            &f16,
            net::bearer_for(&url, endpoint, token),
            None,
        )?;
    }
    quantize_gguf(&f16, &q6, &quantizer, "q6_k", options)?;
    remove_if_exists(&f16)?;
    Ok(q6)
}

fn run_official_nemo(
    options: &FetchOptions,
    endpoint: &str,
    token: Option<&str>,
    keep_nemo: bool,
    keep_f16: bool,
) -> Result<PathBuf> {
    let paths = FetchPaths::new_prepared()?;
    run_official_nemo_with_paths(options, endpoint, token, keep_nemo, keep_f16, paths)
}

fn run_official_nemo_with_paths(
    options: &FetchOptions,
    endpoint: &str,
    token: Option<&str>,
    keep_nemo: bool,
    keep_f16: bool,
    paths: FetchPaths,
) -> Result<PathBuf> {
    // Conversion and quantization use deterministic intermediate names. Hold
    // the same destination lock as the hosted download for the whole source
    // transaction so another fetch cannot remove, rename, or overwrite them.
    let _lock = acquire_artifact_lock(&paths.q8)?;
    if !options.force && paths.q8.is_file() {
        options.verbose_status(format_args!(
            "parakit: using cached model: {}",
            paths.q8.display()
        ));
        cleanup_intermediates(&paths, keep_nemo, keep_f16)?;
        return Ok(paths.q8);
    }

    let converter_script = converter_script_path();
    if !converter_script.is_file() {
        bail!(
            "converter script not found at {}. Run from a full parakit checkout with the CrispASR submodule initialized.",
            converter_script.display()
        );
    }
    let quantize_bin = quantize_bin_path()?;
    let nemo_url = net::rewrite_pinned_url(OFFICIAL_NEMO_URL, endpoint);

    if options.force {
        remove_if_exists(&paths.nemo)?;
        remove_if_exists(&paths.f16)?;
    }

    let preflight_python = if paths.f16.is_file() {
        None
    } else {
        Some(python_with_converter_deps()?)
    };

    ensure_nemo(&paths, options, &nemo_url, endpoint, token)?;
    ensure_f16(
        &paths,
        &converter_script,
        preflight_python.as_deref(),
        options,
    )?;
    ensure_q8(&paths.f16, &paths.q8, &quantize_bin, options)?;

    cleanup_intermediates(&paths, keep_nemo, keep_f16)?;
    Ok(paths.q8)
}

fn run_url(options: &FetchOptions, url: &str, expected_sha: Option<&str>) -> Result<PathBuf> {
    let file_name = url_file_name(url)?;
    let dir = models_dir()?;
    let dest_dir = url_cache_dir(&dir, url);
    std::fs::create_dir_all(&dest_dir).with_context(|| format!("create {}", dest_dir.display()))?;
    let dest = dest_dir.join(&file_name);

    if !options.force && use_cached_download(options, &dest, expected_sha)? {
        return Ok(dest);
    }

    let client = net::build_client()?;
    options.downloading(url);
    // A user-supplied --url is never sent the Hub bearer token, even if it
    // happens to point at the resolved Hub endpoint host: the token is only
    // for requests parakit itself builds against the Hub.
    download_and_verify(options, &client, url, &dest, None, expected_sha)?;

    Ok(dest)
}

fn url_cache_dir(models_dir: &Path, url: &str) -> PathBuf {
    let key = crate::checksum::hex_digest(&Sha256::digest(url.as_bytes()));
    models_dir.join("url").join(key)
}

/// Use a cached model download when the destination exists and, when
/// the user supplied one, its SHA256 matches `expected_sha`.
///
/// Shared by hosted, direct-URL, and Hugging Face repo downloads.
///
/// # Arguments
///
/// * `options` - Shared fetch options; source of the verbose status line.
/// * `dest` - The destination path to check.
/// * `expected_sha` - Expected SHA256, if known.
///
/// # Returns
///
/// `true` when `dest` exists and, when `expected_sha` is supplied, matches it.
///
/// # Errors
///
/// Returns an error if the user supplied `expected_sha` and `dest` cannot be
/// hashed.
fn use_cached_download(
    options: &FetchOptions,
    dest: &Path,
    expected_sha: Option<&str>,
) -> Result<bool> {
    if !dest.is_file() {
        return Ok(false);
    }
    if let Some(expected) = expected_sha {
        if crate::checksum::sha256_file_hex(dest)? != expected {
            return Ok(false);
        }
    }
    options.verbose_status(format_args!(
        "parakit: using cached model: {}",
        dest.display()
    ));
    Ok(true)
}

fn url_file_name(url: &str) -> Result<String> {
    let without_extras = url.split(['?', '#']).next().unwrap_or(url);
    let name = without_extras.rsplit(['/', '\\']).next().unwrap_or("");
    validate_download_file_name(name)
        .with_context(|| format!("URL has no usable file name segment: {url}"))?;
    Ok(name.to_string())
}

fn validate_download_file_name(name: &str) -> Result<()> {
    if name.is_empty() || matches!(name, "." | "..") {
        bail!("file name segment is empty or relative");
    }
    if name.ends_with([' ', '.'])
        || name.chars().any(|c| {
            c <= '\u{1f}' || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
        })
    {
        bail!("file name '{name}' contains characters that are invalid on Windows");
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end_matches(' ')
        .to_uppercase();
    let numbered_device = ["COM", "LPT"].iter().any(|prefix| {
        stem.strip_prefix(prefix).is_some_and(|suffix| {
            let mut chars = suffix.chars();
            matches!(
                (chars.next(), chars.next()),
                (Some('1'..='9' | '¹' | '²' | '³'), None)
            )
        })
    });
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL") || numbered_device {
        bail!("file name '{name}' is reserved on Windows");
    }
    Ok(())
}

fn toml_path_literal(path: &Path) -> String {
    toml::Value::String(path.to_string_lossy().into_owned()).to_string()
}

/// Download `url` to `dest` (via a `.gguf.part` staging file), verifying
/// against `expected_sha` when known.
///
/// # Errors
///
/// Returns an error if the download fails or does not match `expected_sha`.
fn download_and_verify(
    options: &FetchOptions,
    client: &Client,
    url: &str,
    dest: &Path,
    bearer: Option<&str>,
    expected_sha: Option<&str>,
) -> Result<()> {
    let _lock = acquire_artifact_lock(dest)?;
    if !options.force && use_cached_download(options, dest, expected_sha)? {
        return Ok(());
    }

    let partial = partial_path(dest);
    if options.force {
        remove_if_exists(&partial)?;
    }
    net::download_with_resume(client, url, &partial, bearer, options.quiet)?;
    if let Some(expected) = expected_sha {
        let sha = crate::checksum::sha256_file_hex(&partial)?;
        if sha != expected {
            remove_if_exists(&partial)?;
            bail!("downloaded file checksum mismatch for {url}: expected {expected}, got {sha}");
        }
    }

    move_into_place(&partial, dest)?;
    Ok(())
}

fn acquire_artifact_lock(dest: &Path) -> Result<File> {
    let lock_path = artifact_sidecar_path(dest, ".lock");
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("open download lock {}", lock_path.display()))?;
    lock.lock_exclusive()
        .with_context(|| format!("lock download destination {}", dest.display()))?;
    Ok(lock)
}

fn partial_path(dest: &Path) -> PathBuf {
    artifact_sidecar_path(dest, ".part")
}

fn artifact_sidecar_path(dest: &Path, suffix: &str) -> PathBuf {
    let mut path = dest.as_os_str().to_os_string();
    path.push(suffix);
    PathBuf::from(path)
}

#[derive(Debug)]
struct FetchPaths {
    nemo: PathBuf,
    f16: PathBuf,
    q8: PathBuf,
}

impl FetchPaths {
    fn new_prepared() -> Result<Self> {
        let models_dir = models_dir()?;
        std::fs::create_dir_all(&models_dir)
            .with_context(|| format!("create {}", models_dir.display()))?;
        Ok(Self {
            nemo: models_dir.join(NEMO_FILENAME),
            f16: models_dir.join(F16_FILENAME),
            q8: models_dir.join(Q8_FILENAME),
        })
    }
}

fn ensure_nemo(
    paths: &FetchPaths,
    options: &FetchOptions,
    nemo_url: &str,
    endpoint: &str,
    token: Option<&str>,
) -> Result<()> {
    if paths.nemo.is_file() {
        options.status(format_args!(
            "parakit: using cached checkpoint: {}",
            paths.nemo.display()
        ));
        return Ok(());
    }

    let client = net::build_client()?;
    let bearer = net::bearer_for(nemo_url, endpoint, token);
    options.downloading(nemo_url);
    download_and_verify(options, &client, nemo_url, &paths.nemo, bearer, None)?;
    Ok(())
}

fn ensure_f16(
    paths: &FetchPaths,
    converter_script: &Path,
    preflight_python: Option<&Path>,
    options: &FetchOptions,
) -> Result<()> {
    if paths.f16.is_file() {
        options.status(format_args!(
            "parakit: using cached F16 GGUF: {}",
            paths.f16.display()
        ));
        return Ok(());
    }

    // TODO(convert-rust-port): replace the Python converter when the Parakeet
    // `.nemo` to GGUF path has a maintained Rust implementation.
    let python = match preflight_python {
        Some(path) => path.to_path_buf(),
        None => python_with_converter_deps()?,
    };
    let tmp_f16 = paths.f16.with_extension("gguf.converting");
    remove_if_exists(&tmp_f16)?;
    if let Err(err) = options.run_command(
        Command::new(&python)
            .arg(converter_script)
            .arg("--nemo")
            .arg(&paths.nemo)
            .arg("--output")
            .arg(&tmp_f16),
        "converting checkpoint",
    ) {
        let _ = remove_if_exists(&tmp_f16);
        return Err(err);
    }
    move_into_place(&tmp_f16, &paths.f16)?;
    Ok(())
}

fn ensure_q8(f16: &Path, q8: &Path, quantize_bin: &Path, options: &FetchOptions) -> Result<()> {
    quantize_gguf(f16, q8, quantize_bin, "q8_0", options)
}

fn quantize_gguf(
    f16: &Path,
    dest: &Path,
    quantize_bin: &Path,
    quant: &str,
    options: &FetchOptions,
) -> Result<()> {
    let staging = dest.with_extension("gguf.quantizing");
    remove_if_exists(&staging)?;
    let mut command = Command::new(quantize_bin);
    command
        .arg(f16)
        .arg(&staging)
        .arg(quant)
        .env("CRISPASR_QWEN3ASR_QUANT_AUDIO", "0");
    add_bundled_library_path(&mut command, quantize_bin);
    if let Err(err) = options.run_command(
        &mut command,
        &format!("preparing {} model", quant.to_ascii_uppercase()),
    ) {
        let _ = remove_if_exists(&staging);
        return Err(err);
    }

    move_into_place(&staging, dest)?;
    Ok(())
}

fn python_with_converter_deps() -> Result<PathBuf> {
    let python = find_python()?;
    let status = Command::new(&python)
        .arg("-c")
        .arg("import gguf, numpy, sentencepiece, torch")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("run {}", python.display()))?;
    if status.success() {
        return Ok(python);
    }

    bail!(
        "Conversion requires Python 3 with torch, numpy, gguf, and sentencepiece. Install with: pip install -r scripts/requirements-convert.txt"
    );
}

fn find_python() -> Result<PathBuf> {
    if command_works("python3", ["--version"]) {
        return Ok(PathBuf::from("python3"));
    }
    if command_works("python", ["--version"]) {
        return Ok(PathBuf::from("python"));
    }
    bail!(
        "Conversion requires Python 3. Install Python from https://www.python.org/downloads/ and then run: pip install -r scripts/requirements-convert.txt"
    );
}

fn quantize_bin_path() -> Result<PathBuf> {
    if let Some(path) = option_env!("CRISPASR_QUANTIZE_BIN") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Ok(path);
        }
    }

    if let Some(install_dir) = option_env!("CRISPASR_INSTALL_DIR") {
        let candidate = PathBuf::from(install_dir)
            .join("bin")
            .join(exe_name("crispasr-quantize"));
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    if let Some(path) = find_on_path(exe_name("crispasr-quantize")) {
        return Ok(path);
    }

    #[cfg(target_os = "windows")]
    bail!(
        "crispasr-quantize.exe was not found. Windows bundled CPU builds skip the CrispASR examples tree because the pinned server example does not compile under MSVC. Use the hosted Q8 model, or put a compatible crispasr-quantize.exe on PATH before running fetch --from-source or --quantize q6_k."
    );

    #[cfg(not(target_os = "windows"))]
    bail!(
        "crispasr-quantize was not found. Rebuild parakit with bundled CrispASR enabled, or put crispasr-quantize on PATH."
    );
}

fn converter_script_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("vendor")
        .join("CrispASR")
        .join("models")
        .join("convert-parakeet-to-gguf.py")
}

fn add_bundled_library_path(command: &mut Command, executable: &Path) {
    let Some(install_dir) = executable.parent().and_then(Path::parent) else {
        return;
    };
    let lib_dir = install_dir.join("lib");
    if lib_dir.is_dir() {
        prepend_env_path(command, dynamic_library_path_var(), &lib_dir);
    }
}

fn dynamic_library_path_var() -> &'static str {
    if cfg!(target_os = "windows") {
        "PATH"
    } else if cfg!(target_os = "macos") {
        "DYLD_LIBRARY_PATH"
    } else {
        "LD_LIBRARY_PATH"
    }
}

fn prepend_env_path(command: &mut Command, key: &str, dir: &Path) {
    if let Some(joined) = joined_path_with_prepended(dir, env::var_os(key)) {
        command.env(key, joined);
    }
}

fn joined_path_with_prepended(dir: &Path, existing: Option<OsString>) -> Option<OsString> {
    let mut paths = vec![dir.to_path_buf()];
    if let Some(existing) = existing {
        paths.extend(env::split_paths(&existing));
    }
    env::join_paths(paths).ok()
}

fn run_command(command: &mut Command, label: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("spawn command to {label}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("{label} failed with status {status}"))
    }
}

/// Move a completed download into place.
///
/// POSIX `rename` overwrites an existing destination atomically; Windows
/// `rename` cannot, so the destination is removed first there.
fn move_into_place(src: &Path, dst: &Path) -> Result<()> {
    #[cfg(not(unix))]
    remove_if_exists(dst)?;

    std::fs::rename(src, dst).with_context(|| {
        format!(
            "move completed model from {} to {}",
            src.display(),
            dst.display()
        )
    })
}

fn cleanup_intermediates(paths: &FetchPaths, keep_nemo: bool, keep_f16: bool) -> Result<()> {
    if !keep_nemo {
        remove_if_exists(&paths.nemo)?;
    }
    if !keep_f16 {
        remove_if_exists(&paths.f16)?;
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("remove {}", path.display())),
    }
}

fn command_works<I, S>(program: &str, args: I) -> bool
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn find_on_path(name: String) -> Option<PathBuf> {
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|dir| dir.join(&name))
        .find(|path| path.is_file())
}

fn exe_name(name: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
