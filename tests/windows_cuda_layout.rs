//! Integration coverage for Windows CUDA runtime DLL layout detection.

mod common;
#[path = "../build/windows_cuda.rs"]
mod windows_cuda;

use std::fs;

use windows_cuda::{
    cuda_external_dll_names, cuda_runtime_dirs, derive_cuda_external_dll_names,
    discover_cuda_external_dll_names, display_paths, is_cuda_external_dll_name,
};

#[test]
fn derives_cuda_runtime_dll_names_from_toolkit_major_as_fallback() {
    assert_eq!(
        derive_cuda_external_dll_names("13.2"),
        vec!["cudart64_13.dll", "cublas64_13.dll", "cublasLt64_13.dll"]
    );
    assert_eq!(
        derive_cuda_external_dll_names("Cuda compilation tools, release 12.6, V12.6.85"),
        vec!["cudart64_12.dll", "cublas64_12.dll", "cublasLt64_12.dll"]
    );
    assert!(derive_cuda_external_dll_names("unknown").is_empty());
}

#[test]
fn discovers_cuda_runtime_dlls_from_bin_x64_without_version_assumptions() {
    let root = common::fixture_root("windows-cuda-layout-tests", "bin-x64");
    common::touch(&root.join("bin/x64/cublasLt64_99.dll"));
    common::touch(&root.join("bin/x64/cudart64_99.dll"));
    common::touch(&root.join("bin/x64/cublas64_99.dll"));
    common::touch(&root.join("bin/x64/nvrtc64_990_0.dll"));

    assert_eq!(
        discover_cuda_external_dll_names(&root),
        vec!["cudart64_99.dll", "cublas64_99.dll", "cublasLt64_99.dll"]
    );
}

#[test]
fn resolved_cuda_dll_names_prefer_discovered_toolkit_files() {
    let root = common::fixture_root("windows-cuda-layout-tests", "prefer-discovered");
    common::touch(&root.join("bin/cudart64_42.dll"));
    common::touch(&root.join("bin/cublas64_42.dll"));
    common::touch(&root.join("bin/cublasLt64_42.dll"));

    assert_eq!(
        cuda_external_dll_names(Some(&root), "13.2"),
        vec!["cudart64_42.dll", "cublas64_42.dll", "cublasLt64_42.dll"]
    );
}

#[test]
fn resolved_cuda_dll_names_fall_back_to_toolkit_major_when_discovery_is_empty() {
    let root = common::fixture_root("windows-cuda-layout-tests", "fallback");
    fs::create_dir_all(root.join("bin")).expect("fixture bin dir should be created");

    assert_eq!(
        cuda_external_dll_names(Some(&root), "13.2"),
        vec!["cudart64_13.dll", "cublas64_13.dll", "cublasLt64_13.dll"]
    );
}

#[test]
fn cuda_runtime_dirs_include_bin_and_bin_x64_layouts() {
    let root = common::fixture_root("windows-cuda-layout-tests", "runtime-dirs");
    fs::create_dir_all(root.join("bin/x64")).expect("fixture bin/x64 dir should be created");

    assert_eq!(
        cuda_runtime_dirs(&root),
        vec![root.join("bin"), root.join("bin/x64")]
    );
}

#[test]
fn cuda_runtime_dll_filter_accepts_required_runtime_names_only() {
    assert!(is_cuda_external_dll_name("cudart64_13.dll"));
    assert!(is_cuda_external_dll_name("cublas64_13.dll"));
    assert!(is_cuda_external_dll_name("cublasLt64_13.dll"));
    assert!(!is_cuda_external_dll_name("nvrtc64_130_0.dll"));
    assert!(!is_cuda_external_dll_name("cublas64_13.lib"));
    assert!(!is_cuda_external_dll_name("unrelated.dll"));
}

#[test]
fn display_paths_joins_candidate_dirs_for_errors() {
    assert_eq!(
        display_paths(&[
            std::path::PathBuf::from("C:\\CUDA\\bin"),
            std::path::PathBuf::from("C:\\CUDA\\bin\\x64")
        ]),
        "C:\\CUDA\\bin, C:\\CUDA\\bin\\x64"
    );
}
