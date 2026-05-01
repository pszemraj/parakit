//! Model acquisition for the default Parakeet GGUF and source rebuilds.

use crate::model::{
    models_dir, F16_FILENAME, HOSTED_Q8_SHA256, HOSTED_Q8_URL, MANIFEST_FILENAME, NEMO_FILENAME,
    OFFICIAL_NEMO_URL, Q8_FILENAME,
};
use anyhow::{anyhow, bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, RANGE, USER_AGENT};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const ACQ_HOSTED_Q8: &str = "hosted-q8";
const ACQ_OFFICIAL_NEMO: &str = "official-nemo";

/// Options for `parakit fetch`.
#[derive(Clone, Copy, Debug)]
pub struct FetchOptions {
    /// Ignore existing cache entries and rebuild all artifacts.
    pub force: bool,
    /// Suppress stdout status messages.
    pub quiet: bool,
    /// Which acquisition path to use.
    pub source: FetchSource,
}

/// Model acquisition source for `parakit fetch`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FetchSource {
    /// Download the owner-hosted Q8_0 GGUF.
    HostedQ8,
    /// Rebuild Q8_0 locally from NVIDIA's official `.nemo` checkpoint.
    OfficialNemo {
        /// Keep the downloaded `.nemo` after the final Q8_0 model is produced.
        keep_nemo: bool,
        /// Keep the intermediate F16 GGUF after the final Q8_0 model is produced.
        keep_f16: bool,
    },
}

/// Ensure the default hosted Q8_0 model is present in the cache.
///
/// # Returns
///
/// The canonical cached Q8_0 model path.
///
/// # Errors
///
/// Returns an error if the model cannot be downloaded, verified, or written
/// into the platform cache directory.
pub fn ensure_default_model(quiet: bool) -> Result<PathBuf> {
    run(FetchOptions {
        force: false,
        quiet,
        source: FetchSource::HostedQ8,
    })
}

/// Run a model acquisition pipeline.
///
/// # Returns
///
/// The canonical cached Q8_0 model path.
///
/// # Errors
///
/// Returns an error if the model cannot be downloaded, verified, converted,
/// quantized, or written into the platform cache directory.
pub fn run(options: FetchOptions) -> Result<PathBuf> {
    match options.source {
        FetchSource::HostedQ8 => run_hosted_q8(options),
        FetchSource::OfficialNemo { .. } => run_official_nemo(options),
    }
}

fn run_hosted_q8(options: FetchOptions) -> Result<PathBuf> {
    let paths = FetchPaths::new()?;
    std::fs::create_dir_all(&paths.models_dir)
        .with_context(|| format!("create {}", paths.models_dir.display()))?;
    let mut manifest = Manifest::load(&paths.manifest)?.unwrap_or_default();
    let partial = paths.q8.with_extension("gguf.part");

    if options.force {
        remove_if_exists(&paths.q8)?;
        remove_if_exists(&partial)?;
    } else if paths.q8.is_file() {
        if manifest.hosted_current(&paths.q8) {
            status(
                options,
                format_args!("parakit: cached model is current: {}", paths.q8.display()),
            );
            return Ok(paths.q8);
        }

        let current = crate::checksum::sha256_file_hex(&paths.q8)?;
        if current == HOSTED_Q8_SHA256 {
            manifest.mark_hosted_ready(&paths.q8);
            manifest.save(&paths.manifest)?;
            status(
                options,
                format_args!("parakit: cached model is current: {}", paths.q8.display()),
            );
            return Ok(paths.q8);
        }
        status(
            options,
            format_args!(
                "parakit: cached model checksum mismatch, replacing: {}",
                paths.q8.display()
            ),
        );
        remove_if_exists(&paths.q8)?;
    }

    status(
        options,
        format_args!("parakit: downloading {}", HOSTED_Q8_URL),
    );
    download_with_resume(HOSTED_Q8_URL, &partial)?;
    let mut downloaded_sha = crate::checksum::sha256_file_hex(&partial)?;
    if downloaded_sha != HOSTED_Q8_SHA256 {
        status(
            options,
            format_args!("parakit: downloaded partial checksum mismatch, restarting download"),
        );
        remove_if_exists(&partial)?;
        download_with_resume(HOSTED_Q8_URL, &partial)?;
        downloaded_sha = crate::checksum::sha256_file_hex(&partial)?;
    }
    if downloaded_sha != HOSTED_Q8_SHA256 {
        remove_if_exists(&partial)?;
        bail!(
            "downloaded model checksum mismatch for {}: expected {}, got {}",
            HOSTED_Q8_URL,
            HOSTED_Q8_SHA256,
            downloaded_sha
        );
    }

    move_into_place(&partial, &paths.q8)?;
    manifest.mark_hosted_ready(&paths.q8);
    manifest.save(&paths.manifest)?;
    status(
        options,
        format_args!("parakit: model ready: {}", paths.q8.display()),
    );
    Ok(paths.q8)
}

fn run_official_nemo(options: FetchOptions) -> Result<PathBuf> {
    let paths = FetchPaths::new()?;
    std::fs::create_dir_all(&paths.models_dir)
        .with_context(|| format!("create {}", paths.models_dir.display()))?;

    let converter_script = converter_script_path();
    if !converter_script.is_file() {
        bail!(
            "converter script not found at {}. Run from a full parakit checkout with the CrispASR submodule initialized.",
            converter_script.display()
        );
    }
    let crispasr_sha = crispasr_git_sha();
    let quantize_bin = quantize_bin_path()?;
    let quantize_version = quantize_version(&quantize_bin, &crispasr_sha);
    let mut manifest = Manifest::load(&paths.manifest)?.unwrap_or_default();

    if options.force {
        remove_if_exists(&paths.nemo)?;
        remove_if_exists(&paths.f16)?;
        remove_if_exists(&paths.q8)?;
    } else if manifest.final_current(
        &paths.q8,
        &converter_script,
        &crispasr_sha,
        &quantize_bin,
        &quantize_version,
    )? {
        status(
            options,
            format_args!(
                "parakit: cached source-built model is current: {}",
                paths.q8.display()
            ),
        );
        cleanup_intermediates(&paths, options)?;
        return Ok(paths.q8);
    }

    let preflight_python = if paths.f16.is_file() {
        None
    } else {
        Some(python_with_converter_deps()?)
    };

    let nemo_sha = ensure_nemo(&paths, &mut manifest, options)?;
    manifest.save(&paths.manifest)?;

    let f16_sha = ensure_f16(
        &paths,
        &mut manifest,
        &converter_script,
        &crispasr_sha,
        &nemo_sha,
        preflight_python.as_deref(),
        options,
    )?;
    manifest.save(&paths.manifest)?;

    ensure_q8(
        &paths,
        &mut manifest,
        &quantize_bin,
        &quantize_version,
        &f16_sha,
        options,
    )?;
    manifest.save(&paths.manifest)?;

    cleanup_intermediates(&paths, options)?;
    status(
        options,
        format_args!("parakit: model ready: {}", paths.q8.display()),
    );
    Ok(paths.q8)
}

#[derive(Debug)]
struct FetchPaths {
    models_dir: PathBuf,
    manifest: PathBuf,
    nemo: PathBuf,
    f16: PathBuf,
    q8: PathBuf,
}

impl FetchPaths {
    fn new() -> Result<Self> {
        let models_dir = models_dir()?;
        Ok(Self {
            manifest: models_dir.join(MANIFEST_FILENAME),
            nemo: models_dir.join(NEMO_FILENAME),
            f16: models_dir.join(F16_FILENAME),
            q8: models_dir.join(Q8_FILENAME),
            models_dir,
        })
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct Manifest {
    acquisition: String,
    source_url: String,
    nemo_sha256: Option<String>,
    f16_input_sha256: Option<String>,
    f16_sha256: Option<String>,
    q8_input_sha256: Option<String>,
    q8_sha256: Option<String>,
    q8_output_path: String,
    converter_script: String,
    converter_crispasr_git_sha: String,
    crispasr_quantize_bin: String,
    crispasr_quantize_version: String,
    downloaded_at: Option<String>,
    converted_at: Option<String>,
    quantized_at: Option<String>,
}

impl Manifest {
    fn load(path: &Path) -> Result<Option<Self>> {
        if !path.is_file() {
            return Ok(None);
        }
        let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
        let manifest =
            serde_json::from_reader(file).with_context(|| format!("parse {}", path.display()))?;
        Ok(Some(manifest))
    }

    fn save(&self, path: &Path) -> Result<()> {
        let mut file = File::create(path).with_context(|| format!("create {}", path.display()))?;
        serde_json::to_writer_pretty(&mut file, self)?;
        file.write_all(b"\n")?;
        file.flush()?;
        Ok(())
    }

    fn hosted_current(&self, q8_path: &Path) -> bool {
        self.acquisition == ACQ_HOSTED_Q8
            && self.source_url == HOSTED_Q8_URL
            && self.q8_sha256.as_deref() == Some(HOSTED_Q8_SHA256)
            && self.q8_output_path == q8_path.display().to_string()
    }

    fn mark_hosted_ready(&mut self, q8_path: &Path) {
        self.acquisition = ACQ_HOSTED_Q8.to_string();
        self.source_url = HOSTED_Q8_URL.to_string();
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

    fn final_current(
        &self,
        q8_path: &Path,
        converter_script: &Path,
        crispasr_sha: &str,
        quantize_bin: &Path,
        quantize_version: &str,
    ) -> Result<bool> {
        if self.acquisition != ACQ_OFFICIAL_NEMO
            || self.source_url != OFFICIAL_NEMO_URL
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
}

fn ensure_nemo(
    paths: &FetchPaths,
    manifest: &mut Manifest,
    options: FetchOptions,
) -> Result<String> {
    if paths.nemo.is_file() {
        let current = crate::checksum::sha256_file_hex(&paths.nemo)?;
        if manifest.source_url == OFFICIAL_NEMO_URL
            && manifest.nemo_sha256.as_deref() == Some(&current)
        {
            status(
                options,
                format_args!("parakit: using cached checkpoint: {}", paths.nemo.display()),
            );
            return Ok(current);
        }
    }

    status(
        options,
        format_args!("parakit: downloading {}", OFFICIAL_NEMO_URL),
    );
    download_with_resume(OFFICIAL_NEMO_URL, &paths.nemo)?;
    let sha = crate::checksum::sha256_file_hex(&paths.nemo)?;
    manifest.acquisition = ACQ_OFFICIAL_NEMO.to_string();
    manifest.source_url = OFFICIAL_NEMO_URL.to_string();
    manifest.nemo_sha256 = Some(sha.clone());
    manifest.downloaded_at = Some(now_utc());
    Ok(sha)
}

fn ensure_f16(
    paths: &FetchPaths,
    manifest: &mut Manifest,
    converter_script: &Path,
    crispasr_sha: &str,
    nemo_sha: &str,
    preflight_python: Option<&Path>,
    options: FetchOptions,
) -> Result<String> {
    if paths.f16.is_file()
        && manifest.f16_input_sha256.as_deref() == Some(nemo_sha)
        && manifest.converter_script == converter_script.display().to_string()
        && manifest.converter_crispasr_git_sha == crispasr_sha
    {
        let current = crate::checksum::sha256_file_hex(&paths.f16)?;
        if manifest.f16_sha256.as_deref() == Some(&current) {
            status(
                options,
                format_args!("parakit: using cached F16 GGUF: {}", paths.f16.display()),
            );
            return Ok(current);
        }
    }

    // TODO(convert-rust-port): replace the Python converter when the Parakeet
    // `.nemo` to GGUF path has a maintained Rust implementation.
    let python = match preflight_python {
        Some(path) => path.to_path_buf(),
        None => python_with_converter_deps()?,
    };
    remove_if_exists(&paths.f16)?;
    status(
        options,
        format_args!("parakit: converting .nemo to F16 GGUF"),
    );
    run_command(
        Command::new(&python)
            .arg(converter_script)
            .arg("--nemo")
            .arg(&paths.nemo)
            .arg("--output")
            .arg(&paths.f16),
        "convert Parakeet .nemo to GGUF",
    )?;

    let f16_sha = crate::checksum::sha256_file_hex(&paths.f16)?;
    manifest.f16_input_sha256 = Some(nemo_sha.to_string());
    manifest.f16_sha256 = Some(f16_sha.clone());
    manifest.converter_script = converter_script.display().to_string();
    manifest.converter_crispasr_git_sha = crispasr_sha.to_string();
    manifest.converted_at = Some(now_utc());
    Ok(f16_sha)
}

fn ensure_q8(
    paths: &FetchPaths,
    manifest: &mut Manifest,
    quantize_bin: &Path,
    quantize_version: &str,
    f16_sha: &str,
    options: FetchOptions,
) -> Result<String> {
    if paths.q8.is_file()
        && manifest.q8_input_sha256.as_deref() == Some(f16_sha)
        && manifest.crispasr_quantize_bin == quantize_bin.display().to_string()
        && manifest.crispasr_quantize_version == quantize_version
    {
        let current = crate::checksum::sha256_file_hex(&paths.q8)?;
        if manifest.q8_sha256.as_deref() == Some(&current) {
            status(
                options,
                format_args!("parakit: using cached Q8_0 GGUF: {}", paths.q8.display()),
            );
            return Ok(current);
        }
    }

    remove_if_exists(&paths.q8)?;
    status(
        options,
        format_args!("parakit: quantizing F16 GGUF to Q8_0"),
    );
    let mut command = Command::new(quantize_bin);
    command.arg(&paths.f16).arg(&paths.q8).arg("q8_0");
    add_bundled_library_path(&mut command, quantize_bin);
    run_command(&mut command, "quantize GGUF to Q8_0")?;

    let q8_sha = crate::checksum::sha256_file_hex(&paths.q8)?;
    manifest.q8_input_sha256 = Some(f16_sha.to_string());
    manifest.q8_sha256 = Some(q8_sha.clone());
    manifest.q8_output_path = paths.q8.display().to_string();
    manifest.crispasr_quantize_bin = quantize_bin.display().to_string();
    manifest.crispasr_quantize_version = quantize_version.to_string();
    manifest.quantized_at = Some(now_utc());
    Ok(q8_sha)
}

fn download_with_resume(url: &str, path: &Path) -> Result<()> {
    let client = Client::builder()
        .default_headers(default_headers())
        .build()
        .context("build HTTP client")?;

    let mut start = path.metadata().map(|m| m.len()).unwrap_or(0);
    let mut request = client.get(url);
    if start > 0 {
        request = request.header(RANGE, format!("bytes={start}-"));
    }

    let mut response = request.send().with_context(|| format!("GET {url}"))?;
    match response.status() {
        StatusCode::OK => {
            if start > 0 {
                start = 0;
            }
        }
        StatusCode::PARTIAL_CONTENT => {}
        StatusCode::RANGE_NOT_SATISFIABLE => {
            remove_if_exists(path)?;
            response = client
                .get(url)
                .send()
                .with_context(|| format!("GET {url}"))?;
            if response.status() != StatusCode::OK {
                bail!(
                    "download restart failed with HTTP status {}",
                    response.status()
                );
            }
            start = 0;
        }
        status => {
            bail!("download failed with HTTP status {status}");
        }
    }

    let mut file = if start == 0 {
        File::create(path).with_context(|| format!("create {}", path.display()))?
    } else {
        OpenOptions::new()
            .append(true)
            .open(path)
            .with_context(|| format!("open {}", path.display()))?
    };
    std::io::copy(&mut response, &mut file)?;
    file.flush()?;
    Ok(())
}

fn default_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(concat!("parakit/", env!("CARGO_PKG_VERSION"))),
    );
    headers
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

fn crispasr_git_sha() -> String {
    let vendor = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("vendor")
        .join("CrispASR");
    Command::new("git")
        .arg("-C")
        .arg(vendor)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn quantize_version(quantize_bin: &Path, crispasr_sha: &str) -> String {
    let metadata = quantize_bin
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|mtime| mtime.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|| "unknown-mtime".to_string());
    format!("crispasr {crispasr_sha}; binary-mtime {metadata}")
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

fn move_into_place(src: &Path, dst: &Path) -> Result<()> {
    remove_if_exists(dst)?;
    std::fs::rename(src, dst).with_context(|| {
        format!(
            "move verified model from {} to {}",
            src.display(),
            dst.display()
        )
    })
}

fn status(options: FetchOptions, message: std::fmt::Arguments<'_>) {
    if !options.quiet {
        println!("{message}");
    }
}

fn cleanup_intermediates(paths: &FetchPaths, options: FetchOptions) -> Result<()> {
    if let FetchSource::OfficialNemo {
        keep_nemo,
        keep_f16,
    } = options.source
    {
        if !keep_nemo {
            remove_if_exists(&paths.nemo)?;
        }
        if !keep_f16 {
            remove_if_exists(&paths.f16)?;
        }
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

fn now_utc() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_manifest_records_default_model() {
        let mut manifest = Manifest::default();
        let path = Path::new("target/tmp/parakit-fetch-tests/model.gguf");
        manifest.mark_hosted_ready(path);

        assert!(manifest.hosted_current(path));
        assert_eq!(manifest.acquisition, ACQ_HOSTED_Q8);
        assert_eq!(manifest.source_url, HOSTED_Q8_URL);
        assert_eq!(manifest.q8_sha256.as_deref(), Some(HOSTED_Q8_SHA256));
        assert!(manifest.nemo_sha256.is_none());
        assert!(manifest.f16_sha256.is_none());
    }

    #[test]
    fn move_into_place_replaces_existing_file() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/tmp/parakit-fetch-tests/move-into-place");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("model.gguf.part");
        let dst = dir.join("model.gguf");
        std::fs::write(&src, b"new").unwrap();
        std::fs::write(&dst, b"old").unwrap();

        move_into_place(&src, &dst).unwrap();

        assert!(!src.exists());
        assert_eq!(std::fs::read(&dst).unwrap(), b"new");
    }

    #[test]
    fn prepend_env_path_keeps_existing_entries() {
        let dir = Path::new("target/tmp/parakit-fetch-tests/lib");
        let existing =
            env::join_paths([Path::new("target/tmp/a"), Path::new("target/tmp/b")]).unwrap();
        let joined = joined_path_with_prepended(dir, Some(existing)).unwrap();
        let paths: Vec<_> = env::split_paths(&joined).collect();

        assert_eq!(paths[0], dir);
        assert_eq!(paths[1], Path::new("target/tmp/a"));
        assert_eq!(paths[2], Path::new("target/tmp/b"));
    }
}
