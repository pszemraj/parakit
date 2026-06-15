//! Integration coverage for Windows OpenBLAS layout detection.

mod common;
#[path = "../build/windows_openblas.rs"]
mod windows_openblas;

use windows_openblas::{
    find_windows_openblas, is_known_openblas_runtime_dll, WindowsOpenBlasImportKind,
};

#[test]
fn detects_current_conda_openblas_layout() {
    let root = common::fixture_root_with_files(
        "windows-openblas-layout-tests",
        "current-conda",
        &[
            "include/openblas/cblas.h",
            "lib/openblas.lib",
            "bin/openblas.dll",
        ],
    );

    let found = find_windows_openblas(&root, WindowsOpenBlasImportKind::Msvc)
        .expect("layout should be detected");

    assert_eq!(found.include_dir, root.join("include/openblas"));
    assert_eq!(found.import_lib, root.join("lib/openblas.lib"));
    assert!(found.runtime_dlls.contains(&root.join("bin/openblas.dll")));
}

#[test]
fn detects_flat_include_and_libopenblas_dll_layout() {
    let root = common::fixture_root_with_files(
        "windows-openblas-layout-tests",
        "flat-libopenblas",
        &[
            "include/cblas.h",
            "lib/libopenblas.lib",
            "bin/libopenblas.dll",
            "bin/libomp.dll",
        ],
    );

    let found = find_windows_openblas(&root, WindowsOpenBlasImportKind::Msvc)
        .expect("layout should be detected");

    assert_eq!(found.include_dir, root.join("include"));
    assert_eq!(found.import_lib, root.join("lib/libopenblas.lib"));
    assert_eq!(
        found.runtime_dlls,
        vec![
            root.join("bin/libomp.dll"),
            root.join("bin/libopenblas.dll")
        ]
    );
}

#[test]
fn detects_gnu_import_lib_and_versioned_runtime_layout() {
    let root = common::fixture_root_with_files(
        "windows-openblas-layout-tests",
        "gnu-versioned",
        &[
            "include/cblas.h",
            "lib/libopenblas.dll.a",
            "bin/libopenblas64_.dll",
            "bin/libgcc_s_seh-1.dll",
            "bin/libwinpthread-1.dll",
        ],
    );

    let found = find_windows_openblas(&root, WindowsOpenBlasImportKind::Gnu)
        .expect("layout should be detected");

    assert_eq!(found.import_lib, root.join("lib/libopenblas.dll.a"));
    assert!(found
        .runtime_dlls
        .contains(&root.join("bin/libopenblas64_.dll")));
    assert!(found
        .runtime_dlls
        .contains(&root.join("bin/libwinpthread-1.dll")));
}

#[test]
fn rejects_layout_without_primary_runtime_dll() {
    let root = common::fixture_root_with_files(
        "windows-openblas-layout-tests",
        "missing-primary-runtime",
        &["include/cblas.h", "lib/openblas.lib", "bin/libomp.dll"],
    );

    assert!(find_windows_openblas(&root, WindowsOpenBlasImportKind::Msvc).is_none());
}

#[test]
fn msvc_rejects_gnu_only_import_lib_layout() {
    let root = common::fixture_root_with_files(
        "windows-openblas-layout-tests",
        "msvc-rejects-dll-a",
        &[
            "include/cblas.h",
            "lib/libopenblas.dll.a",
            "bin/libopenblas.dll",
        ],
    );

    assert!(find_windows_openblas(&root, WindowsOpenBlasImportKind::Msvc).is_none());
}

#[test]
fn target_kind_selects_compatible_import_library_from_mixed_layout() {
    let root = common::fixture_root_with_files(
        "windows-openblas-layout-tests",
        "mixed-import-libs",
        &[
            "include/cblas.h",
            "lib/openblas.lib",
            "lib/libopenblas.dll.a",
            "bin/libopenblas.dll",
        ],
    );

    let msvc = find_windows_openblas(&root, WindowsOpenBlasImportKind::Msvc)
        .expect("MSVC layout should be detected");
    let gnu = find_windows_openblas(&root, WindowsOpenBlasImportKind::Gnu)
        .expect("GNU layout should be detected");

    assert_eq!(msvc.import_lib, root.join("lib/openblas.lib"));
    assert_eq!(gnu.import_lib, root.join("lib/libopenblas.dll.a"));
}

#[test]
fn runtime_dll_filter_accepts_primary_and_known_dependency_names() {
    assert!(is_known_openblas_runtime_dll("openblas.dll"));
    assert!(is_known_openblas_runtime_dll("libopenblas.dll"));
    assert!(is_known_openblas_runtime_dll("libopenblas64_.dll"));
    assert!(is_known_openblas_runtime_dll("libgfortran-5.dll"));
    assert!(is_known_openblas_runtime_dll("libgcc_s_seh-1.dll"));
    assert!(is_known_openblas_runtime_dll("libquadmath-0.dll"));
    assert!(is_known_openblas_runtime_dll("libwinpthread-1.dll"));
    assert!(!is_known_openblas_runtime_dll("unrelated.dll"));
}
