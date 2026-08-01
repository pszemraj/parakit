//! Common OpenBLAS prefix candidates for Windows and Linux builds.

use std::env;
use std::path::PathBuf;

/// Return OpenBLAS roots named by common environment and package-manager
/// conventions.
///
/// # Arguments
///
/// * `windows` - Whether Windows prefix conventions should be used.
///
/// # Returns
///
/// Candidate roots in precedence order, without duplicates.
pub(crate) fn configured_openblas_roots(windows: bool) -> Vec<PathBuf> {
    let mut roots = Vec::new();

    for name in ["OPENBLAS_ROOT", "OpenBLAS_ROOT", "OPENBLAS_HOME"] {
        if let Some(root) = env_path(name) {
            push_openblas_root(&mut roots, root, windows);
        }
    }
    if let Some(conda) = env_path("CONDA_PREFIX") {
        push_openblas_root(&mut roots, conda, windows);
    }
    if let Some(prefixes) = env::var_os("CMAKE_PREFIX_PATH") {
        for prefix in env::split_paths(&prefixes) {
            push_openblas_root(&mut roots, prefix, windows);
        }
    }

    let triplet = vcpkg_triplet(windows);
    for name in ["VCPKG_ROOT", "VCPKG_INSTALLATION_ROOT"] {
        if let Some(root) = env_path(name) {
            push_unique_path(&mut roots, root.join("installed").join(&triplet), windows);
        }
    }
    if let Some(installed) = env_path("VCPKG_INSTALLED_DIR") {
        push_unique_path(&mut roots, installed.join(&triplet), windows);
    }

    if !windows {
        if let Some(prefix) = env_path("HOMEBREW_PREFIX") {
            push_unique_path(&mut roots, prefix, windows);
        }
    }

    roots
}

/// Return conventional unmanaged OpenBLAS installation roots.
///
/// # Arguments
///
/// * `windows` - Whether Windows prefix conventions should be used.
///
/// # Returns
///
/// Candidate roots in precedence order, without duplicates.
pub(crate) fn conventional_openblas_roots(windows: bool) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if windows {
        for name in ["USERPROFILE", "PROGRAMDATA"] {
            if let Some(base) = env_path(name) {
                for distribution in ["miniforge3", "miniconda3", "anaconda3"] {
                    push_unique_path(&mut roots, base.join(distribution).join("Library"), windows);
                }
            }
        }
        if let Some(program_files) = env_path("ProgramFiles") {
            push_unique_path(&mut roots, program_files.join("OpenBLAS"), windows);
        }
        if let Some(system_drive) = env_path("SystemDrive") {
            push_unique_path(
                &mut roots,
                PathBuf::from(format!(r"{}\OpenBLAS", system_drive.display())),
                windows,
            );
        }
    } else {
        for root in ["/usr/local", "/usr", "/opt/openblas", "/opt/OpenBLAS"] {
            push_unique_path(&mut roots, PathBuf::from(root), windows);
        }
    }
    roots
}

fn push_openblas_root(roots: &mut Vec<PathBuf>, root: PathBuf, windows: bool) {
    if windows {
        push_unique_path(roots, root.join("Library"), windows);
    }
    push_unique_path(roots, root, windows);
}

fn push_unique_path(paths: &mut Vec<PathBuf>, path: PathBuf, windows: bool) {
    let duplicate = paths.iter().any(|existing| {
        if windows {
            existing
                .to_string_lossy()
                .eq_ignore_ascii_case(&path.to_string_lossy())
        } else {
            existing == &path
        }
    });
    if !duplicate {
        paths.push(path);
    }
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn vcpkg_triplet(windows: bool) -> String {
    if let Ok(triplet) = env::var("VCPKG_DEFAULT_TRIPLET") {
        if !triplet.trim().is_empty() {
            return triplet;
        }
    }

    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let architecture = match target_arch.as_str() {
        "x86_64" => "x64",
        "x86" => "x86",
        "aarch64" => "arm64",
        other => other,
    };
    format!(
        "{architecture}-{}",
        if windows { "windows" } else { "linux" }
    )
}
