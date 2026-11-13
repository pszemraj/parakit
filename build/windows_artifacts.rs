//! Windows native artifact staging used by the build script.

use super::{
    manual_blas_path_overrides_are_set, read_cmake_cache, AcceleratorConfig, BlasConfig,
    OpenBlasInstall,
};
use crate::windows_cuda::{cuda_external_dll_names, cuda_runtime_dirs, remove_cuda_external_dlls};
use crate::windows_manifest::{Accelerator, CudaManifest, RuntimeManifest, VulkanManifest};
use crate::windows_openblas::{remove_known_openblas_runtime_dlls, WindowsOpenBlas};
use std::env;
use std::path::{Path, PathBuf};

/// File name for the runtime manifest colocated with `parakit.exe`.
const RUNTIME_MANIFEST: &str = "parakit-runtime-manifest.json";

/// Stage external Windows runtimes and their manifest inside OUT_DIR.
///
/// # Arguments
///
/// * `install_dir` - CMake installation root to collect.
/// * `bin_dir` - Destination for runtime DLLs and the manifest.
/// * `blas` - Resolved BLAS selection and installation details.
/// * `accelerators` - Enabled accelerator backends and detected versions.
///
/// # Panics
///
/// Panics when required CrispASR artifacts are missing or an artifact cannot
/// be copied or written.
pub(crate) fn prepare_windows_artifacts(
    install_dir: &Path,
    bin_dir: &Path,
    blas: &BlasConfig,
    accelerators: &AcceleratorConfig,
) {
    std::fs::create_dir_all(bin_dir).unwrap_or_else(|err| {
        panic!(
            "failed to create Windows runtime DLL dir {}: {err}",
            bin_dir.display()
        )
    });

    remove_known_openblas_runtime_dlls(bin_dir).unwrap_or_else(|err| {
        panic!(
            "failed to remove stale OpenBLAS runtime DLLs from {}: {err}",
            bin_dir.display()
        )
    });
    remove_cuda_external_dlls(bin_dir).unwrap_or_else(|err| {
        panic!(
            "failed to remove stale CUDA runtime DLLs from {}: {err}",
            bin_dir.display()
        )
    });
    let cuda_manifest = cuda_manifest(install_dir, bin_dir, accelerators);
    let vulkan_manifest = vulkan_manifest(accelerators);

    copy_optional_windows_blas_runtime(bin_dir, blas);
    let runtime_dlls = windows_runtime_dll_names_for_bundle(bin_dir);
    write_windows_runtime_manifest(
        bin_dir,
        &runtime_dlls,
        accelerators,
        cuda_manifest,
        vulkan_manifest,
    );
}

fn copy_optional_windows_blas_runtime(bin_dir: &Path, blas: &BlasConfig) {
    if blas.selected != "openblas" {
        return;
    }

    let Some(openblas) = windows_openblas_for_bundle(blas) else {
        return;
    };
    for path in &openblas.runtime_dlls {
        let Some(name) = path.file_name() else {
            continue;
        };
        let dest = bin_dir.join(name);
        copy_file_or_panic(path, &dest, "Windows OpenBLAS runtime DLL");
    }
}

fn cuda_manifest(
    install_dir: &Path,
    bin_dir: &Path,
    accelerators: &AcceleratorConfig,
) -> Option<CudaManifest> {
    if !accelerators.cuda_enabled {
        return None;
    }

    let toolkit_version = accelerators
        .cuda_toolkit_version
        .clone()
        .unwrap_or_else(|| "unknown".to_string());
    let architectures = cmake_cache_value(install_dir, "CMAKE_CUDA_ARCHITECTURES")
        .or_else(|| accelerators.cuda_archs_request.clone())
        .unwrap_or_else(|| "native".to_string());
    let cuda_path = env::var_os("CUDA_PATH").map(PathBuf::from);
    let external_dlls = cuda_external_dll_names(cuda_path.as_deref(), &toolkit_version);
    let external_dlls_bundled = env_flag_enabled("PARAKIT_BUNDLE_CUDA_DLLS");
    if external_dlls_bundled {
        copy_cuda_external_dlls(bin_dir, cuda_path.as_deref(), &external_dlls);
    }

    Some(CudaManifest {
        toolkit_version,
        architectures,
        external_dlls,
        external_dlls_bundled,
    })
}

fn vulkan_manifest(accelerators: &AcceleratorConfig) -> Option<VulkanManifest> {
    if !accelerators.vulkan_enabled {
        return None;
    }

    Some(VulkanManifest {
        sdk_version: accelerators
            .vulkan_sdk_version
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
        external_dlls: vec!["vulkan-1.dll".to_string()],
        external_dlls_bundled: false,
    })
}

fn cmake_cache_value(install_dir: &Path, key: &str) -> Option<String> {
    let cache = read_cmake_cache(&install_dir.join("build/CMakeCache.txt"));
    cache.get(key).cloned().filter(|value| !value.is_empty())
}

fn env_flag_enabled(name: &str) -> bool {
    env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn copy_cuda_external_dlls(bin_dir: &Path, cuda_path: Option<&Path>, names: &[String]) {
    let Some(cuda_path) = cuda_path else {
        panic!("PARAKIT_BUNDLE_CUDA_DLLS=1 requires CUDA_PATH to point at a CUDA Toolkit install");
    };
    let runtime_dirs = cuda_runtime_dirs(cuda_path);
    if runtime_dirs.is_empty() {
        panic!("PARAKIT_BUNDLE_CUDA_DLLS=1 requires CUDA_PATH to point at a CUDA Toolkit install");
    }
    for name in names {
        let source = runtime_dirs
            .iter()
            .map(|runtime_dir| runtime_dir.join(name))
            .find(|path| path.is_file());
        let Some(source) = source else {
            let searched = runtime_dirs
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            panic!(
                "PARAKIT_BUNDLE_CUDA_DLLS=1 could not find CUDA runtime DLL {name} under {searched}"
            );
        };
        let dest = bin_dir.join(name);
        copy_file_or_panic(&source, &dest, "CUDA runtime DLL");
    }
}

fn windows_openblas_for_bundle(blas: &BlasConfig) -> Option<&WindowsOpenBlas> {
    if manual_blas_path_overrides_are_set() {
        return None;
    }
    match blas.openblas_install.as_ref() {
        Some(OpenBlasInstall::Windows(openblas)) => Some(openblas),
        _ => None,
    }
}

fn copy_file_or_panic(source: &Path, dest: &Path, description: &str) {
    std::fs::copy(source, dest).unwrap_or_else(|err| {
        panic!(
            "failed to copy {description} {} to {}: {err}",
            source.display(),
            dest.display()
        )
    });
}

fn windows_runtime_dll_names_for_bundle(bin_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(bin_dir) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file()
            || !path
                .extension()
                .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("dll"))
        {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if should_skip_windows_bundle_dll(name) {
            continue;
        }
        names.push(name.to_string());
    }
    names.sort();
    names.dedup();
    names
}

fn should_skip_windows_bundle_dll(file_name: &str) -> bool {
    let lower = file_name.to_ascii_lowercase();
    lower.starts_with("ggml-cpu-") && lower.ends_with(".dll")
}

fn write_windows_runtime_manifest(
    bin_dir: &Path,
    runtime_dlls: &[String],
    accelerators: &AcceleratorConfig,
    cuda: Option<CudaManifest>,
    vulkan: Option<VulkanManifest>,
) {
    let mut required_files = Vec::with_capacity(runtime_dlls.len() + 1);
    required_files.push("parakit.exe".to_string());
    required_files.extend(runtime_dlls.iter().cloned());

    let accelerator = if accelerators.cuda_enabled {
        Accelerator::Cuda
    } else if accelerators.vulkan_enabled {
        Accelerator::Vulkan
    } else {
        Accelerator::Cpu
    };
    let manifest = RuntimeManifest {
        required_files,
        accelerator,
        cuda,
        vulkan,
    };
    let path = bin_dir.join(RUNTIME_MANIFEST);
    std::fs::write(&path, manifest.to_json()).unwrap_or_else(|err| {
        panic!(
            "failed to write Windows runtime manifest {}: {err}",
            path.display()
        )
    });
}
