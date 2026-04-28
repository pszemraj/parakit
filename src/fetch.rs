//! Model acquisition pipeline for the official NVIDIA Parakeet checkpoint.

use crate::model::{
    models_dir, F16_FILENAME, MANIFEST_FILENAME, NEMO_FILENAME, Q8_FILENAME, SOURCE_NEMO_URL,
};
use anyhow::{anyhow, bail, Context, Result};
use chrono::{SecondsFormat, Utc};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderValue, RANGE, USER_AGENT};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Options for `parakit fetch`.
#[derive(Clone, Copy, Debug)]
pub struct FetchOptions {
    /// Ignore existing cache entries and rebuild all artifacts.
    pub force: bool,
    /// Keep the downloaded `.nemo` after the final Q8_0 model is produced.
    pub keep_nemo: bool,
    /// Keep the intermediate F16 GGUF after the final Q8_0 model is produced.
    pub keep_f16: bool,
}

/// Run the full official-checkpoint to Q8_0 GGUF pipeline.
///
/// # Returns
///
/// The canonical cached Q8_0 model path.
///
/// # Errors
///
/// Returns an error if the model cannot be downloaded, converted, quantized, or
/// written into the platform cache directory.
pub fn run(options: FetchOptions) -> Result<PathBuf> {
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
        println!("parakit: cached model is current: {}", paths.q8.display());
        cleanup_intermediates(&paths, options)?;
        return Ok(paths.q8);
    }

    let preflight_python = if paths.f16.is_file() {
        None
    } else {
        Some(python_with_converter_deps()?)
    };

    let nemo_sha = ensure_nemo(&paths, &mut manifest)?;
    manifest.save(&paths.manifest)?;

    let f16_sha = ensure_f16(
        &paths,
        &mut manifest,
        &converter_script,
        &crispasr_sha,
        &nemo_sha,
        preflight_python.as_deref(),
    )?;
    manifest.save(&paths.manifest)?;

    let q8_sha = ensure_q8(
        &paths,
        &mut manifest,
        &quantize_bin,
        &quantize_version,
        &f16_sha,
    )?;
    manifest.q8_sha256 = Some(q8_sha);
    manifest.save(&paths.manifest)?;

    cleanup_intermediates(&paths, options)?;
    println!("parakit: model ready: {}", paths.q8.display());
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
struct Manifest {
    source_url: String,
    nemo_sha256: Option<String>,
    f16_input_sha256: Option<String>,
    f16_sha256: Option<String>,
    q8_input_sha256: Option<String>,
    q8_sha256: Option<String>,
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

    fn final_current(
        &self,
        q8_path: &Path,
        converter_script: &Path,
        crispasr_sha: &str,
        quantize_bin: &Path,
        quantize_version: &str,
    ) -> Result<bool> {
        if self.source_url != SOURCE_NEMO_URL || !q8_path.is_file() {
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
        Ok(hash_file(q8_path)? == *recorded_q8)
    }
}

fn ensure_nemo(paths: &FetchPaths, manifest: &mut Manifest) -> Result<String> {
    if paths.nemo.is_file() {
        let current = hash_file(&paths.nemo)?;
        if manifest.source_url == SOURCE_NEMO_URL
            && manifest.nemo_sha256.as_deref() == Some(&current)
        {
            println!("parakit: using cached checkpoint: {}", paths.nemo.display());
            return Ok(current);
        }
    }

    println!("parakit: downloading {}", SOURCE_NEMO_URL);
    download_with_resume(SOURCE_NEMO_URL, &paths.nemo)?;
    let sha = hash_file(&paths.nemo)?;
    manifest.source_url = SOURCE_NEMO_URL.to_string();
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
) -> Result<String> {
    if paths.f16.is_file()
        && manifest.f16_input_sha256.as_deref() == Some(nemo_sha)
        && manifest.converter_script == converter_script.display().to_string()
        && manifest.converter_crispasr_git_sha == crispasr_sha
    {
        let current = hash_file(&paths.f16)?;
        if manifest.f16_sha256.as_deref() == Some(&current) {
            println!("parakit: using cached F16 GGUF: {}", paths.f16.display());
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
    println!("parakit: converting .nemo to F16 GGUF");
    run_command(
        Command::new(&python)
            .arg(converter_script)
            .arg("--nemo")
            .arg(&paths.nemo)
            .arg("--output")
            .arg(&paths.f16),
        "convert Parakeet .nemo to GGUF",
    )?;

    let f16_sha = hash_file(&paths.f16)?;
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
) -> Result<String> {
    if paths.q8.is_file()
        && manifest.q8_input_sha256.as_deref() == Some(f16_sha)
        && manifest.crispasr_quantize_bin == quantize_bin.display().to_string()
        && manifest.crispasr_quantize_version == quantize_version
    {
        let current = hash_file(&paths.q8)?;
        if manifest.q8_sha256.as_deref() == Some(&current) {
            println!("parakit: using cached Q8_0 GGUF: {}", paths.q8.display());
            return Ok(current);
        }
    }

    remove_if_exists(&paths.q8)?;
    println!("parakit: quantizing F16 GGUF to Q8_0");
    run_command(
        Command::new(quantize_bin)
            .arg(&paths.f16)
            .arg(&paths.q8)
            .arg("q8_0"),
        "quantize GGUF to Q8_0",
    )?;

    let q8_sha = hash_file(&paths.q8)?;
    manifest.q8_input_sha256 = Some(f16_sha.to_string());
    manifest.q8_sha256 = Some(q8_sha.clone());
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
        "Conversion requires Python 3 with torch, numpy, gguf, and sentencepiece. Install with: pip install -r requirements-convert.txt"
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
        "Conversion requires Python 3. Install Python from https://www.python.org/downloads/ and then run: pip install -r requirements-convert.txt"
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

fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0_u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_digest(&hasher.finalize()))
}

fn hex_digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn cleanup_intermediates(paths: &FetchPaths, options: FetchOptions) -> Result<()> {
    if !options.keep_nemo {
        remove_if_exists(&paths.nemo)?;
    }
    if !options.keep_f16 {
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
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
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
    fn sha256_hex_formatter_is_lowercase() {
        assert_eq!(hex_digest(&[0, 10, 255]), "000aff");
    }

    #[test]
    fn fetch_options_are_copyable() {
        let opts = FetchOptions {
            force: false,
            keep_nemo: true,
            keep_f16: false,
        };
        let copy = opts;
        assert!(copy.keep_nemo);
    }
}
