//! Windows native artifact staging used by the build script.

use super::{
    manual_blas_path_overrides_are_set, read_cmake_cache, AcceleratorConfig, BlasConfig,
    OpenBlasInstall,
};
use crate::windows_cuda::{cuda_external_dll_names, cuda_runtime_dirs};
use crate::windows_manifest::{
    stale_runtime_dlls, Accelerator, CudaManifest, RuntimeManifest, VulkanManifest,
};
use crate::windows_openblas::WindowsOpenBlas;
use std::env;
use std::path::{Path, PathBuf};

/// File name for the runtime manifest colocated with `parakit.exe`.
const RUNTIME_MANIFEST: &str = "parakit-runtime-manifest.json";

/// Collect native Windows outputs, write their runtime manifest, and stage the
/// resulting flat bundle beside the Cargo profile executable.
///
/// # Arguments
///
/// * `install_dir` - CMake installation root to collect.
/// * `lib_dir` - Destination for import libraries.
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
    lib_dir: &Path,
    bin_dir: &Path,
    blas: &BlasConfig,
    accelerators: &AcceleratorConfig,
) {
    std::fs::create_dir_all(lib_dir).unwrap_or_else(|err| {
        panic!(
            "failed to create Windows import-library dir {}: {err}",
            lib_dir.display()
        )
    });
    std::fs::create_dir_all(bin_dir).unwrap_or_else(|err| {
        panic!(
            "failed to create Windows runtime DLL dir {}: {err}",
            bin_dir.display()
        )
    });

    copy_windows_runtime_dlls(install_dir, bin_dir);
    let cuda_manifest = cuda_manifest(install_dir, bin_dir, accelerators);
    let vulkan_manifest = vulkan_manifest(accelerators);

    let crispasr_import_library = windows_import_library_name("crispasr");
    let ggml_import_library = windows_import_library_name("ggml");
    copy_named_artifact(install_dir, &crispasr_import_library, lib_dir);
    copy_named_artifact(install_dir, &ggml_import_library, lib_dir);
    copy_named_artifact(install_dir, "crispasr.dll", bin_dir);

    copy_optional_windows_blas_runtime(bin_dir, blas);
    let runtime_dlls = windows_runtime_dll_names_for_bundle(bin_dir);
    write_windows_runtime_manifest(
        bin_dir,
        &runtime_dlls,
        accelerators,
        cuda_manifest,
        vulkan_manifest,
    );
    copy_profile_artifacts(bin_dir, &runtime_dlls);
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

fn windows_import_library_name(base: &str) -> String {
    if env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default() == "gnu" {
        format!("lib{base}.dll.a")
    } else {
        format!("{base}.lib")
    }
}

fn copy_windows_runtime_dlls(install_dir: &Path, bin_dir: &Path) {
    let mut dlls = collect_files_with_extension(&install_dir.join("build"), "dll");
    dlls.extend(collect_files_with_extension(bin_dir, "dll"));
    dlls.sort();
    dlls.dedup();

    for dll in dlls {
        let Some(name) = dll.file_name() else {
            continue;
        };
        let dest = bin_dir.join(name);
        if dll != dest {
            copy_file_or_panic(&dll, &dest, "Windows runtime DLL");
        }
    }
}

fn copy_named_artifact(install_dir: &Path, file_name: &str, dest_dir: &Path) {
    let dest = dest_dir.join(file_name);
    let mut matches = collect_files_named(install_dir, file_name);
    matches.sort();

    let Some(src) = matches
        .into_iter()
        .find(|path| path != &dest)
        .or_else(|| dest.is_file().then_some(dest.clone()))
    else {
        panic!(
            "CrispASR build did not produce {file_name}. Check the Windows CMake output with `cargo build -vv`."
        );
    };

    if src != dest {
        copy_file_or_panic(&src, &dest, "Windows build artifact");
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

fn copy_profile_artifacts(bin_dir: &Path, runtime_dlls: &[String]) {
    let Some(profile_dir) = cargo_profile_dir() else {
        return;
    };

    remove_stale_profile_dlls(&profile_dir, runtime_dlls);

    if let Ok(entries) = std::fs::read_dir(bin_dir) {
        for path in entries.flatten().map(|entry| entry.path()) {
            if !path
                .extension()
                .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case("dll"))
            {
                continue;
            }
            let Some(name) = path.file_name() else {
                continue;
            };
            copy_file_or_panic(&path, &profile_dir.join(name), "runtime DLL");
        }
    }

    let manifest = bin_dir.join(RUNTIME_MANIFEST);
    if manifest.is_file() {
        copy_file_or_panic(
            &manifest,
            &profile_dir.join(RUNTIME_MANIFEST),
            "runtime manifest",
        );
    }
}

fn remove_stale_profile_dlls(profile_dir: &Path, current_dlls: &[String]) {
    let manifest = profile_dir.join(RUNTIME_MANIFEST);
    let Ok(previous_json) = std::fs::read_to_string(manifest) else {
        return;
    };

    for name in stale_runtime_dlls(&previous_json, current_dlls) {
        let path = profile_dir.join(&name);
        if !path.is_file() {
            continue;
        }
        std::fs::remove_file(&path).unwrap_or_else(|err| {
            panic!(
                "failed to remove stale Windows runtime DLL {}: {err}",
                path.display()
            )
        });
    }
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

fn cargo_profile_dir() -> Option<PathBuf> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").ok()?);
    let build_dir = out_dir.parent()?.parent()?;
    if build_dir.file_name()? != "build" {
        return None;
    }
    build_dir.parent().map(Path::to_path_buf)
}

fn collect_files_named(root: &Path, file_name: &str) -> Vec<PathBuf> {
    collect_files(root, &|path| {
        path.file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(file_name))
    })
}

fn collect_files_with_extension(root: &Path, extension: &str) -> Vec<PathBuf> {
    collect_files(root, &|path| {
        path.extension()
            .is_some_and(|ext| ext.to_string_lossy().eq_ignore_ascii_case(extension))
    })
}

fn collect_files(root: &Path, matches: &impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files.extend(collect_files(&path, matches));
        } else if path.is_file() && matches(&path) {
            files.push(path);
        }
    }
    files
}
