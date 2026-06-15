//! Windows CUDA runtime DLL layout detection used by the build script.
//!
//! The helper is kept outside `build.rs` so version and directory probing can
//! be tested without running the full native build script.

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

const CUDA_RUNTIME_DLL_PREFIXES: &[&str] = &["cudart64_", "cublas64_", "cublaslt64_"];

/// Resolve the CUDA runtime DLL names that should be available to the bundle.
///
/// # Arguments
///
/// * `cuda_path` - Optional CUDA Toolkit root.
/// * `toolkit_version` - CUDA Toolkit version string reported by the build.
///
/// # Returns
///
/// DLL names discovered from the installed toolkit when possible, otherwise
/// names derived from the toolkit major version.
pub(crate) fn cuda_external_dll_names(
    cuda_path: Option<&Path>,
    toolkit_version: &str,
) -> Vec<String> {
    let discovered = cuda_path
        .map(discover_cuda_external_dll_names)
        .unwrap_or_default();
    if !discovered.is_empty() {
        return discovered;
    }

    derive_cuda_external_dll_names(toolkit_version)
}

/// Derive CUDA runtime DLL names from a CUDA Toolkit version string.
///
/// # Returns
///
/// The expected `cudart64_<major>.dll`, `cublas64_<major>.dll`,
/// and `cublasLt64_<major>.dll` names, or an empty vector when no numeric
/// major version can be parsed.
pub(crate) fn derive_cuda_external_dll_names(toolkit_version: &str) -> Vec<String> {
    let Some(major) = toolkit_version
        .split(|ch: char| !ch.is_ascii_digit())
        .find(|part| !part.is_empty())
    else {
        return Vec::new();
    };
    vec![
        format!("cudart64_{major}.dll"),
        format!("cublas64_{major}.dll"),
        format!("cublasLt64_{major}.dll"),
    ]
}

/// Discover CUDA runtime DLL names from known toolkit runtime directories.
///
/// CUDA Toolkit layouts have changed across versions; CUDA 13.x can place
/// runtime DLLs under `bin\x64`, while earlier layouts commonly used `bin`.
///
/// # Returns
///
/// Stable, de-duplicated DLL file names discovered from the toolkit root.
pub(crate) fn discover_cuda_external_dll_names(cuda_path: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for runtime_dir in cuda_runtime_dirs(cuda_path) {
        let Ok(entries) = fs::read_dir(runtime_dir) else {
            continue;
        };
        names.extend(
            entries
                .flatten()
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| is_cuda_external_dll_name(name)),
        );
    }
    sort_cuda_runtime_dll_names(&mut names);
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    names
}

/// Runtime directories that can contain CUDA DLLs for a toolkit root.
///
/// # Returns
///
/// Existing runtime directories in search order.
pub(crate) fn cuda_runtime_dirs(cuda_path: &Path) -> Vec<PathBuf> {
    let bin_dir = cuda_path.join("bin");
    [bin_dir.clone(), bin_dir.join("x64")]
        .into_iter()
        .filter(|path| path.is_dir())
        .collect()
}

/// Return whether `file_name` is a CUDA runtime DLL that the CUDA backend
/// links at process load time.
///
/// # Returns
///
/// `true` for CUDA runtime DLL names that need to be bundled or externally
/// available.
pub(crate) fn is_cuda_external_dll_name(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    lower.ends_with(".dll")
        && CUDA_RUNTIME_DLL_PREFIXES
            .iter()
            .any(|prefix| lower.starts_with(prefix))
}

/// Return a readable list of candidate source directories.
///
/// # Returns
///
/// Candidate paths joined for use in diagnostics.
pub(crate) fn display_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

fn sort_cuda_runtime_dll_names(names: &mut [String]) {
    names.sort_by(|a, b| compare_cuda_runtime_dll_names(a, b));
}

fn compare_cuda_runtime_dll_names(a: &str, b: &str) -> Ordering {
    let a_lower = a.to_ascii_lowercase();
    let b_lower = b.to_ascii_lowercase();
    cuda_runtime_dll_order(&a_lower)
        .cmp(&cuda_runtime_dll_order(&b_lower))
        .then_with(|| a_lower.cmp(&b_lower))
}

fn cuda_runtime_dll_order(file_name: &str) -> usize {
    CUDA_RUNTIME_DLL_PREFIXES
        .iter()
        .position(|prefix| file_name.starts_with(prefix))
        .unwrap_or(CUDA_RUNTIME_DLL_PREFIXES.len())
}
