//! Windows OpenBLAS layout detection used by the build script.
//!
//! The helper is kept outside `build.rs` so its path probing can be tested by
//! the integration test without running the full native build script.

use std::fs;
use std::path::{Path, PathBuf};

/// A target-compatible Windows OpenBLAS installation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WindowsOpenBlas {
    /// OpenBLAS prefix root.
    pub(crate) root: PathBuf,
    /// Directory containing `cblas.h`.
    pub(crate) include_dir: PathBuf,
    /// Import library used by the active Rust target environment.
    pub(crate) import_lib: PathBuf,
    /// Runtime DLLs that should be copied into the bundle.
    pub(crate) runtime_dlls: Vec<PathBuf>,
}

/// Import-library flavor required by the active Windows Rust target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WindowsOpenBlasImportKind {
    /// MSVC-style `.lib` import library.
    Msvc,
    /// GNU-style `.dll.a` import library.
    Gnu,
}

/// Locate a usable Windows OpenBLAS layout under `root`.
///
/// # Arguments
///
/// * `root` - Candidate OpenBLAS prefix containing `include`, `lib`, and `bin`.
/// * `import_kind` - Import-library flavor required by the active target.
///
/// # Returns
///
/// The detected include directory, import library, and runtime DLL set, or
/// `None` when the layout is incomplete or incompatible.
pub(crate) fn find_windows_openblas(
    root: &Path,
    import_kind: WindowsOpenBlasImportKind,
) -> Option<WindowsOpenBlas> {
    let include_dir = [root.join("include/openblas"), root.join("include")]
        .into_iter()
        .find(|dir| dir.join("cblas.h").is_file())?;

    let import_lib = windows_openblas_import_candidates(root, import_kind)
        .into_iter()
        .find(|path| path.is_file())?;

    let runtime_dlls = windows_openblas_runtime_dlls(root);
    if !runtime_dlls.iter().any(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(is_primary_openblas_runtime_dll)
    }) {
        return None;
    }

    Some(WindowsOpenBlas {
        root: root.to_path_buf(),
        include_dir,
        import_lib,
        runtime_dlls,
    })
}

fn windows_openblas_import_candidates(
    root: &Path,
    import_kind: WindowsOpenBlasImportKind,
) -> Vec<PathBuf> {
    match import_kind {
        WindowsOpenBlasImportKind::Msvc => {
            vec![
                root.join("lib/openblas.lib"),
                root.join("lib/libopenblas.lib"),
            ]
        }
        WindowsOpenBlasImportKind::Gnu => vec![
            root.join("lib/libopenblas.dll.a"),
            root.join("lib/openblas.dll.a"),
        ],
    }
}

fn windows_openblas_runtime_dlls(root: &Path) -> Vec<PathBuf> {
    let source_dir = root.join("bin");
    let Ok(entries) = fs::read_dir(source_dir) else {
        return Vec::new();
    };

    let mut dlls = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(is_known_openblas_runtime_dll)
        })
        .collect::<Vec<_>>();
    dlls.sort();
    dlls.dedup();
    dlls
}

/// Return whether `file_name` is an OpenBLAS runtime DLL or known dependency.
///
/// # Arguments
///
/// * `file_name` - DLL file name without a directory component.
///
/// # Returns
///
/// `true` for primary OpenBLAS DLL names and runtime dependency DLLs that need
/// to travel with the Windows bundle.
pub(crate) fn is_known_openblas_runtime_dll(file_name: &str) -> bool {
    is_primary_openblas_runtime_dll(file_name) || is_known_openblas_dependency_dll(file_name)
}

fn is_primary_openblas_runtime_dll(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    lower == "openblas.dll"
        || lower == "libopenblas.dll"
        || dll_name_matches_prefix(&lower, "libopenblas")
}

fn is_known_openblas_dependency_dll(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    lower == "libomp.dll"
        || lower == "libiomp5md.dll"
        || lower == "vcomp140.dll"
        || dll_name_matches_prefix(&lower, "libgfortran")
        || dll_name_matches_prefix(&lower, "libgcc_s_seh")
        || dll_name_matches_prefix(&lower, "libquadmath")
        || dll_name_matches_prefix(&lower, "libwinpthread")
}

fn dll_name_matches_prefix(file_name: &str, prefix: &str) -> bool {
    file_name.starts_with(prefix) && file_name.ends_with(".dll")
}
