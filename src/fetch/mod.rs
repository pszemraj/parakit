//! Model acquisition for `parakit fetch`: the default hosted Q8_0 GGUF, a
//! source rebuild from NVIDIA's official `.nemo` checkpoint, a Hugging Face
//! repo (see [`hub`]), or a direct URL.

mod hub;
mod net;

pub use hub::source_from_cli;

use crate::model::{
    models_dir, F16_FILENAME, HOSTED_Q8_URL, NEMO_FILENAME, OFFICIAL_NEMO_URL, Q8_FILENAME,
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

impl FetchOptions {
    fn status(&self, message: std::fmt::Arguments<'_>) {
        if !self.quiet {
            println!("{message}");
        }
    }

    fn verbose_status(&self, message: std::fmt::Arguments<'_>) {
        if !self.quiet && self.verbose {
            println!("{message}");
        }
    }
}

/// Model acquisition source for `parakit fetch`.
#[derive(Clone, Debug, Eq, PartialEq)]
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
    run(FetchOptions {
        force: false,
        quiet,
        verbose,
        source: FetchSource::HostedQ8,
    })
}

/// Run a model acquisition pipeline.
///
/// # Returns
///
/// The path to the model the requested [`FetchSource`] produced: the
/// canonical cached Q8_0 path for [`FetchSource::HostedQ8`] and
/// [`FetchSource::OfficialNemo`], or the `hub/…`/`url/…` cache path for
/// [`FetchSource::HubRepo`]/[`FetchSource::Url`].
///
/// # Errors
///
/// Returns an error if the model cannot be looked up, downloaded, verified,
/// converted, quantized, or written into the platform cache directory.
pub fn run(options: FetchOptions) -> Result<PathBuf> {
    let endpoint = net::resolve_endpoint();
    let token = net::resolve_token();
    match &options.source {
        FetchSource::HostedQ8 => run_hosted_q8(&options, &endpoint, token.as_deref()),
        FetchSource::OfficialNemo {
            keep_nemo,
            keep_f16,
        } => run_official_nemo(&options, &endpoint, token.as_deref(), *keep_nemo, *keep_f16),
        FetchSource::HubRepo {
            repo,
            revision,
            file,
            sha256,
        } => hub::run_hub_repo(
            &options,
            &endpoint,
            token.as_deref(),
            repo,
            revision.as_deref(),
            file.as_deref(),
            sha256.as_deref(),
        ),
        FetchSource::Url { url, sha256 } => run_url(&options, url, sha256.as_deref()),
    }
}

fn run_hosted_q8(options: &FetchOptions, endpoint: &str, token: Option<&str>) -> Result<PathBuf> {
    let paths = FetchPaths::new_prepared()?;
    let url = net::rewrite_pinned_url(HOSTED_Q8_URL, endpoint);

    if !options.force && paths.q8.is_file() {
        options.verbose_status(format_args!(
            "parakit: using cached model: {}",
            paths.q8.display()
        ));
        return Ok(paths.q8);
    }

    let client = net::build_client()?;
    let bearer = net::bearer_for(&url, endpoint, token);
    options.status(format_args!("parakit: downloading {url}"));
    download_and_verify(options, &client, &url, &paths.q8, bearer, None)?;

    options.status(format_args!("parakit: model ready: {}", paths.q8.display()));
    Ok(paths.q8)
}

fn run_official_nemo(
    options: &FetchOptions,
    endpoint: &str,
    token: Option<&str>,
    keep_nemo: bool,
    keep_f16: bool,
) -> Result<PathBuf> {
    let paths = FetchPaths::new_prepared()?;
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
    ensure_q8(&paths, &quantize_bin, options)?;

    cleanup_intermediates(&paths, keep_nemo, keep_f16)?;
    options.status(format_args!("parakit: model ready: {}", paths.q8.display()));
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
    options.status(format_args!("parakit: downloading {url}"));
    // A user-supplied --url is never sent the Hub bearer token, even if it
    // happens to point at the resolved Hub endpoint host: the token is only
    // for requests parakit itself builds against the Hub.
    download_and_verify(options, &client, url, &dest, None, expected_sha)?;

    print_ready_with_hint(options, &dest);
    Ok(dest)
}

fn url_cache_dir(models_dir: &Path, url: &str) -> PathBuf {
    let key = crate::checksum::hex_digest(&Sha256::digest(url.as_bytes()));
    models_dir.join("url").join(key)
}

/// Use a cached `hub/…`/`url/…` download when the destination exists and, when
/// the user supplied one, its SHA256 matches `expected_sha`.
///
/// Shared by [`run_url`] and `hub::run_hub_repo`.
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

/// Print the three-line "model ready" hint used by non-default fetch sources
/// (Hugging Face repo and direct URL), which — unlike the hosted Q8_0
/// default — are not picked up automatically and need `-m`/`daemon.model` to
/// be used.
fn print_ready_with_hint(options: &FetchOptions, path: &Path) {
    options.status(format_args!("parakit: model ready: {}", path.display()));
    options.status(format_args!(
        "parakit: run `parakit start -m <model-path>` to use it"
    ));
    let config_path = toml_path_literal(path);
    options.status(format_args!(
        "parakit: or set daemon.model = {config_path} in config.toml"
    ));
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
    let _lock = acquire_download_lock(dest)?;
    if !options.force && use_cached_download(options, dest, expected_sha)? {
        return Ok(());
    }

    let partial = partial_path(dest);
    if options.force {
        remove_if_exists(&partial)?;
    }
    net::download_with_resume(client, url, &partial, bearer)?;
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

fn acquire_download_lock(dest: &Path) -> Result<File> {
    let mut lock_path = dest.as_os_str().to_os_string();
    lock_path.push(".lock");
    let lock_path = PathBuf::from(lock_path);
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
    let mut partial = dest.as_os_str().to_os_string();
    partial.push(".part");
    PathBuf::from(partial)
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
    options.status(format_args!("parakit: downloading {nemo_url}"));
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
    options.status(format_args!("parakit: converting .nemo to F16 GGUF"));
    if let Err(err) = run_command(
        Command::new(&python)
            .arg(converter_script)
            .arg("--nemo")
            .arg(&paths.nemo)
            .arg("--output")
            .arg(&tmp_f16),
        "convert Parakeet .nemo to GGUF",
    ) {
        let _ = remove_if_exists(&tmp_f16);
        return Err(err);
    }
    move_into_place(&tmp_f16, &paths.f16)?;
    Ok(())
}

fn ensure_q8(paths: &FetchPaths, quantize_bin: &Path, options: &FetchOptions) -> Result<()> {
    let tmp_q8 = paths.q8.with_extension("gguf.quantizing");
    remove_if_exists(&tmp_q8)?;
    options.status(format_args!("parakit: quantizing F16 GGUF to Q8_0"));
    let mut command = Command::new(quantize_bin);
    command.arg(&paths.f16).arg(&tmp_q8).arg("q8_0");
    add_bundled_library_path(&mut command, quantize_bin);
    if let Err(err) = run_command(&mut command, "quantize GGUF to Q8_0") {
        let _ = remove_if_exists(&tmp_q8);
        return Err(err);
    }

    move_into_place(&tmp_q8, &paths.q8)?;
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
        "crispasr-quantize.exe was not found. Windows bundled CPU builds skip the CrispASR examples tree because the pinned server example does not compile under MSVC. Use the hosted Q8 model, or put a compatible crispasr-quantize.exe on PATH before running fetch --from-source."
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
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Barrier};
    use std::time::{Duration, Instant};

    fn serve_model_once(body: &'static [u8]) -> (String, std::thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
            }
            let request_text = String::from_utf8(request).unwrap();
            let status = if request_text.to_ascii_lowercase().contains("\r\nrange:") {
                "206 Partial Content"
            } else {
                "200 OK"
            };
            write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nETag: \"revision-2\"\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(body).unwrap();
            request_text
        });
        (format!("http://{address}/model.gguf"), handle)
    }

    fn serve_slow_model(body: &'static [u8]) -> (String, std::thread::JoinHandle<usize>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let mut requests = 0;
            let mut deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline && requests < 2 {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(err) => panic!("test server accept failed: {err}"),
                };
                requests += 1;

                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let count = stream.read(&mut buffer).unwrap();
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                }
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"concurrent\"\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .unwrap();
                let split = body.len() / 2;
                stream.write_all(&body[..split]).unwrap();
                stream.flush().unwrap();
                std::thread::sleep(Duration::from_millis(250));
                stream.write_all(&body[split..]).unwrap();
                deadline = Instant::now() + Duration::from_millis(300);
            }
            requests
        });
        (format!("http://{address}/model.gguf"), handle)
    }

    #[test]
    fn move_into_place_replaces_existing_file() {
        let dir = crate::test_support::fixture_root("parakit-fetch-tests", "move-into-place");
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

    #[test]
    fn partial_path_uses_the_gguf_part_staging_convention() {
        assert_eq!(
            partial_path(Path::new("/cache/models/hub/o--r/model-Q8_0.gguf")),
            Path::new("/cache/models/hub/o--r/model-Q8_0.gguf.part")
        );
    }

    #[test]
    fn force_discards_a_resumable_partial_without_hashing_the_download() {
        let dir = crate::test_support::fixture_root("parakit-fetch-tests", "force-partial");
        let dest = dir.join("model.gguf");
        let partial = partial_path(&dest);
        std::fs::write(&partial, b"stale-prefix").unwrap();
        let mut validator = partial.as_os_str().to_os_string();
        validator.push(".validator");
        std::fs::write(PathBuf::from(validator), b"\"revision-1\"").unwrap();

        let (url, server) = serve_model_once(b"fresh-model");
        let options = FetchOptions {
            force: true,
            quiet: true,
            verbose: false,
            source: FetchSource::Url {
                url: url.clone(),
                sha256: None,
            },
        };
        let client = Client::builder().no_proxy().build().unwrap();
        download_and_verify(&options, &client, &url, &dest, None, None).unwrap();
        let request = server.join().unwrap();

        assert!(!request.to_ascii_lowercase().contains("\r\nrange:"));
        assert_eq!(std::fs::read(&dest).unwrap(), b"fresh-model");
    }

    #[test]
    fn concurrent_downloads_share_one_destination_transaction() {
        let dir = crate::test_support::fixture_root("parakit-fetch-tests", "concurrent");
        let dest = dir.join("model.gguf");
        let (url, server) = serve_slow_model(b"complete-model");
        let barrier = Arc::new(Barrier::new(3));

        let downloads = (0..2)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                let url = url.clone();
                let dest = dest.clone();
                std::thread::spawn(move || {
                    let options = FetchOptions {
                        force: false,
                        quiet: true,
                        verbose: false,
                        source: FetchSource::Url {
                            url: url.clone(),
                            sha256: None,
                        },
                    };
                    let client = Client::builder().no_proxy().build().unwrap();
                    barrier.wait();
                    download_and_verify(&options, &client, &url, &dest, None, None)
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();

        for download in downloads {
            download.join().unwrap().unwrap();
        }
        assert_eq!(server.join().unwrap(), 1);
        assert_eq!(std::fs::read(dest).unwrap(), b"complete-model");
    }

    #[test]
    fn explicit_checksum_mismatch_fails_after_one_download() {
        let dir = crate::test_support::fixture_root("parakit-fetch-tests", "sha-mismatch");
        let dest = dir.join("model.gguf");
        let (url, server) = serve_model_once(b"changed-model");
        let options = FetchOptions {
            force: false,
            quiet: true,
            verbose: false,
            source: FetchSource::Url {
                url: url.clone(),
                sha256: Some("0".repeat(64)),
            },
        };
        let client = Client::builder().no_proxy().build().unwrap();

        let error =
            download_and_verify(&options, &client, &url, &dest, None, Some(&"0".repeat(64)))
                .unwrap_err();
        let request = server.join().unwrap();

        assert!(error.to_string().contains("checksum mismatch"));
        assert!(request.starts_with("GET "));
        assert!(!dest.exists());
    }

    #[test]
    fn url_file_name_cases() {
        for (name, url, expect) in [
            (
                "strips a query string",
                "https://example.com/models/model.gguf?download=true",
                Some("model.gguf"),
            ),
            (
                "strips a fragment",
                "https://example.com/models/model.gguf#frag",
                Some("model.gguf"),
            ),
            (
                "treats a backslash as a Windows path separator",
                "https://example.com/models\\nested\\model.gguf",
                Some("model.gguf"),
            ),
            (
                "rejects a URL with no path segment",
                "https://example.com/",
                None,
            ),
            (
                "rejects a URL whose path ends in a slash",
                "https://example.com/models/",
                None,
            ),
            (
                "rejects a parent-directory segment",
                "https://example.com/models/..",
                None,
            ),
            (
                "rejects a Windows device name with an extension",
                "https://example.com/models/CON.gguf",
                None,
            ),
            (
                "rejects a numbered Windows device name case-insensitively",
                "https://example.com/models/com1.GGUF",
                None,
            ),
            (
                "does not reject a device-name prefix",
                "https://example.com/models/computer.gguf",
                Some("computer.gguf"),
            ),
            (
                "does not reject a device number outside the reserved range",
                "https://example.com/models/LPT10.gguf",
                Some("LPT10.gguf"),
            ),
            (
                "rejects a device name before a second extension",
                "https://example.com/models/prn.model.gguf",
                None,
            ),
            (
                "rejects a device name with a superscript digit",
                "https://example.com/models/lpt².gguf",
                None,
            ),
        ] {
            assert_eq!(url_file_name(url).ok().as_deref(), expect, "{name}");
        }
    }

    #[test]
    fn download_file_names_reject_windows_invalid_characters() {
        for name in [
            "model<variant.gguf",
            "model>variant.gguf",
            "model:stream.gguf",
            "model\"variant.gguf",
            "model/variant.gguf",
            "model\\variant.gguf",
            "model|variant.gguf",
            "model?variant.gguf",
            "model*variant.gguf",
            "model.gguf.",
            "model.gguf ",
            "model\u{1f}.gguf",
        ] {
            assert!(
                validate_download_file_name(name).is_err(),
                "{name:?} should be rejected"
            );
        }
    }

    #[test]
    fn config_model_hint_paths_round_trip_through_toml() {
        for path in [
            r"C:\Users\Name With Space\models\model.gguf",
            "target/models/model \"Q8_0\".gguf",
            "target/models/José/model\ncontinued.gguf",
        ] {
            let literal = toml_path_literal(Path::new(path));
            let parsed: toml::Value = format!("daemon.model = {literal}").parse().unwrap();

            assert_eq!(parsed["daemon"]["model"].as_str(), Some(path), "{path:?}");
        }
    }

    #[test]
    fn url_cache_key_includes_the_exact_url() {
        let models = Path::new("target/tmp/models");
        assert_ne!(
            url_cache_dir(models, "https://one.example/model.gguf"),
            url_cache_dir(models, "https://two.example/model.gguf")
        );
        assert_eq!(
            url_cache_dir(models, "https://one.example/model.gguf"),
            url_cache_dir(models, "https://one.example/model.gguf")
        );
    }
}
