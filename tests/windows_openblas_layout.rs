#[path = "../build/windows_openblas.rs"]
mod windows_openblas;

use std::fs;
use std::path::{Path, PathBuf};

use windows_openblas::{
    find_windows_openblas, is_known_openblas_runtime_dll, WindowsOpenBlasImportKind,
};

fn fixture_root(name: &str) -> PathBuf {
    let root = Path::new("target")
        .join("tmp")
        .join("windows-openblas-layout-tests")
        .join(format!("{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    root
}

fn touch(path: &Path) {
    fs::create_dir_all(path.parent().expect("fixture file should have parent"))
        .expect("fixture parent should be created");
    fs::write(path, b"").expect("fixture file should be created");
}

#[test]
fn detects_current_conda_openblas_layout() {
    let root = fixture_root("current-conda");
    touch(&root.join("include/openblas/cblas.h"));
    touch(&root.join("lib/openblas.lib"));
    touch(&root.join("bin/openblas.dll"));

    let found = find_windows_openblas(&root, WindowsOpenBlasImportKind::Msvc)
        .expect("layout should be detected");

    assert_eq!(found.include_dir, root.join("include/openblas"));
    assert_eq!(found.import_lib, root.join("lib/openblas.lib"));
    assert!(found.runtime_dlls.contains(&root.join("bin/openblas.dll")));
}

#[test]
fn detects_flat_include_and_libopenblas_dll_layout() {
    let root = fixture_root("flat-libopenblas");
    touch(&root.join("include/cblas.h"));
    touch(&root.join("lib/libopenblas.lib"));
    touch(&root.join("bin/libopenblas.dll"));
    touch(&root.join("bin/libomp.dll"));

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
    let root = fixture_root("gnu-versioned");
    touch(&root.join("include/cblas.h"));
    touch(&root.join("lib/libopenblas.dll.a"));
    touch(&root.join("bin/libopenblas64_.dll"));
    touch(&root.join("bin/libgcc_s_seh-1.dll"));
    touch(&root.join("bin/libwinpthread-1.dll"));

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
    let root = fixture_root("missing-primary-runtime");
    touch(&root.join("include/cblas.h"));
    touch(&root.join("lib/openblas.lib"));
    touch(&root.join("bin/libomp.dll"));

    assert!(find_windows_openblas(&root, WindowsOpenBlasImportKind::Msvc).is_none());
}

#[test]
fn msvc_rejects_gnu_only_import_lib_layout() {
    let root = fixture_root("msvc-rejects-dll-a");
    touch(&root.join("include/cblas.h"));
    touch(&root.join("lib/libopenblas.dll.a"));
    touch(&root.join("bin/libopenblas.dll"));

    assert!(find_windows_openblas(&root, WindowsOpenBlasImportKind::Msvc).is_none());
}

#[test]
fn target_kind_selects_compatible_import_library_from_mixed_layout() {
    let root = fixture_root("mixed-import-libs");
    touch(&root.join("include/cblas.h"));
    touch(&root.join("lib/openblas.lib"));
    touch(&root.join("lib/libopenblas.dll.a"));
    touch(&root.join("bin/libopenblas.dll"));

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
