//! Unix OpenBLAS layout detection used by the build script.
//!
//! Normal distribution installs are found through `pkg-config`. This helper
//! covers explicit prefixes, source installs, Conda, vcpkg, and conventional
//! system prefixes that do not expose usable pkg-config metadata.

use std::fs;
use std::path::{Path, PathBuf};

/// A usable Unix OpenBLAS installation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UnixOpenBlas {
    /// OpenBLAS prefix root.
    pub(crate) root: PathBuf,
    /// Directory containing `cblas.h`.
    pub(crate) include_dir: PathBuf,
    /// Shared or static OpenBLAS library.
    pub(crate) library: PathBuf,
}

/// Locate a usable Unix OpenBLAS layout under `root`.
///
/// # Arguments
///
/// * `root` - Candidate prefix containing OpenBLAS headers and libraries.
/// * `target_arch` - Cargo target architecture used to filter Linux multiarch directories.
///
/// # Returns
///
/// The detected include directory and library, or `None` when the layout is
/// incomplete.
pub(crate) fn find_unix_openblas(root: &Path, target_arch: &str) -> Option<UnixOpenBlas> {
    let include_dir = unix_openblas_include_dirs(root, target_arch)
        .into_iter()
        .find(|dir| dir.join("cblas.h").is_file())?;
    let library = unix_openblas_library_dirs(root, target_arch)
        .into_iter()
        .find_map(|dir| find_openblas_library(&dir))?;

    Some(UnixOpenBlas {
        root: root.to_path_buf(),
        include_dir,
        library,
    })
}

fn unix_openblas_include_dirs(root: &Path, target_arch: &str) -> Vec<PathBuf> {
    let include = root.join("include");
    let mut dirs = vec![
        include.join("openblas"),
        include.join("openblas-pthread"),
        include.join("openblas-openmp"),
        include.clone(),
    ];

    for child in target_compatible_child_dirs(&include, target_arch) {
        dirs.push(child.join("openblas"));
        dirs.push(child.join("openblas-pthread"));
        dirs.push(child.join("openblas-openmp"));
        dirs.push(child);
    }
    dirs
}

fn unix_openblas_library_dirs(root: &Path, target_arch: &str) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for base in [root.join("lib"), root.join("lib64")] {
        dirs.push(base.clone());
        dirs.extend(target_compatible_child_dirs(&base, target_arch));
    }
    dirs
}

fn target_compatible_child_dirs(parent: &Path, target_arch: &str) -> Vec<PathBuf> {
    child_dirs(parent)
        .into_iter()
        .filter(|path| {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                return true;
            };
            let Some((multiarch_arch, _)) = name.split_once("-linux-") else {
                return true;
            };
            normalize_linux_arch(multiarch_arch) == target_arch
        })
        .collect()
}

fn normalize_linux_arch(arch: &str) -> &str {
    match arch {
        "i386" | "i486" | "i586" | "i686" => "x86",
        "armv6" | "armv7" => "arm",
        "powerpc64le" => "powerpc64",
        other => other,
    }
}

fn child_dirs(parent: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(parent) else {
        return Vec::new();
    };
    let mut dirs = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    dirs.sort();
    dirs
}

fn find_openblas_library(dir: &Path) -> Option<PathBuf> {
    for name in ["libopenblas.so", "libopenblas.dylib", "libopenblas.a"] {
        let path = dir.join(name);
        if path.is_file() {
            return Some(path);
        }
    }

    let Ok(entries) = fs::read_dir(dir) else {
        return None;
    };
    let mut libraries = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_openblas_library_name)
        })
        .collect::<Vec<_>>();
    libraries.sort();
    libraries.into_iter().next()
}

fn is_openblas_library_name(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    lower.starts_with("libopenblas")
        && (lower.ends_with(".a") || lower.ends_with(".dylib") || lower.contains(".so"))
}
