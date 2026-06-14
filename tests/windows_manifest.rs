//! Integration coverage for Windows runtime-manifest serialization.

#[path = "../build/windows_manifest.rs"]
mod windows_manifest;

use serde_json::Value;

use windows_manifest::{
    cuda_external_dll_names, Accelerator, BlasManifest, CudaManifest, RuntimeManifest,
    VulkanManifest, WINDOWS_RUNTIME_MANIFEST,
};

fn base_blas() -> BlasManifest {
    BlasManifest {
        requested: "auto".to_string(),
        selected: "accelerate".to_string(),
        openblas_root: None,
        openblas_include_dir: None,
        openblas_import_lib: None,
        openblas_runtime_dlls: Vec::new(),
    }
}

fn parse(manifest: RuntimeManifest) -> Value {
    serde_json::from_str(&manifest.to_json()).expect("manifest should serialize valid JSON")
}

#[test]
fn serializes_cpu_manifest_with_legacy_fields() {
    let json = parse(RuntimeManifest {
        required_files: vec!["parakit.exe".to_string(), "crispasr.dll".to_string()],
        runtime_dlls: vec!["crispasr.dll".to_string()],
        blas: base_blas(),
        accelerator: Accelerator::Cpu,
        cuda: None,
        vulkan: None,
    });

    assert_eq!(json["required_files"][0], "parakit.exe");
    assert_eq!(json["runtime_dlls"][0], "crispasr.dll");
    assert_eq!(json["blas"]["requested"], "auto");
    assert_eq!(json["blas"]["selected"], "accelerate");
    assert_eq!(json["openblas_root"], Value::Null);
    assert_eq!(json["accelerator"], "cpu");
    assert_eq!(json["cuda"], Value::Null);
    assert_eq!(json["vulkan"], Value::Null);
}

#[test]
fn serializes_cuda_external_dll_contract() {
    let json = parse(RuntimeManifest {
        required_files: vec!["parakit.exe".to_string(), "ggml-cuda.dll".to_string()],
        runtime_dlls: vec!["ggml-cuda.dll".to_string()],
        blas: base_blas(),
        accelerator: Accelerator::Cuda,
        cuda: Some(CudaManifest {
            toolkit_version: "13.2".to_string(),
            architectures: "89-real".to_string(),
            external_dlls: vec![
                "cudart64_13.dll".to_string(),
                "cublas64_13.dll".to_string(),
                "cublasLt64_13.dll".to_string(),
            ],
            external_dlls_bundled: false,
        }),
        vulkan: None,
    });

    assert_eq!(json["accelerator"], "cuda");
    assert_eq!(json["cuda"]["toolkit_version"], "13.2");
    assert_eq!(json["cuda"]["architectures"], "89-real");
    assert_eq!(json["cuda"]["external_dlls"][0], "cudart64_13.dll");
    assert_eq!(json["cuda"]["external_dlls"][1], "cublas64_13.dll");
    assert_eq!(json["cuda"]["external_dlls"][2], "cublasLt64_13.dll");
    assert_eq!(json["cuda"]["external_dlls_bundled"], false);
    assert_eq!(json["vulkan"], Value::Null);
}

#[test]
fn serializes_vulkan_system_loader_contract() {
    let json = parse(RuntimeManifest {
        required_files: vec!["parakit.exe".to_string(), "ggml-vulkan.dll".to_string()],
        runtime_dlls: vec!["ggml-vulkan.dll".to_string()],
        blas: base_blas(),
        accelerator: Accelerator::Vulkan,
        cuda: None,
        vulkan: Some(VulkanManifest {
            sdk_version: "1.4.321.1".to_string(),
            external_dlls: vec!["vulkan-1.dll".to_string()],
            external_dlls_bundled: false,
        }),
    });

    assert_eq!(json["accelerator"], "vulkan");
    assert_eq!(json["cuda"], Value::Null);
    assert_eq!(json["vulkan"]["sdk_version"], "1.4.321.1");
    assert_eq!(json["vulkan"]["external_dlls"][0], "vulkan-1.dll");
    assert_eq!(json["vulkan"]["external_dlls_bundled"], false);
}

#[test]
fn preserves_experimental_multi_backend_metadata() {
    let json = parse(RuntimeManifest {
        required_files: vec!["parakit.exe".to_string()],
        runtime_dlls: Vec::new(),
        blas: base_blas(),
        accelerator: Accelerator::Cuda,
        cuda: Some(CudaManifest {
            toolkit_version: "12.9".to_string(),
            architectures: "native".to_string(),
            external_dlls: cuda_external_dll_names("12.9"),
            external_dlls_bundled: true,
        }),
        vulkan: Some(VulkanManifest {
            sdk_version: "C:\\VulkanSDK\\1.4.321.1".to_string(),
            external_dlls: vec!["vulkan-1.dll".to_string()],
            external_dlls_bundled: false,
        }),
    });

    assert_eq!(json["accelerator"], "cuda");
    assert!(json["cuda"].is_object());
    assert!(json["vulkan"].is_object());
}

#[test]
fn derives_cuda_runtime_dll_names_from_toolkit_major() {
    assert_eq!(
        cuda_external_dll_names("13.2"),
        vec!["cudart64_13.dll", "cublas64_13.dll", "cublasLt64_13.dll"]
    );
    assert_eq!(
        cuda_external_dll_names("Cuda compilation tools, release 12.6, V12.6.85"),
        vec!["cudart64_12.dll", "cublas64_12.dll", "cublasLt64_12.dll"]
    );
    assert!(cuda_external_dll_names("unknown").is_empty());
}

#[test]
fn exposes_runtime_manifest_filename() {
    assert_eq!(WINDOWS_RUNTIME_MANIFEST, "parakit-runtime-manifest.json");
}
