//! Integration coverage for Windows runtime-manifest serialization.

#[path = "../build/windows_manifest.rs"]
mod windows_manifest;

use serde_json::Value;

use windows_manifest::{Accelerator, CudaManifest, RuntimeManifest, VulkanManifest};

fn parse(manifest: RuntimeManifest) -> Value {
    serde_json::from_str(&manifest.to_json()).expect("manifest should serialize valid JSON")
}

#[test]
fn serializes_cpu_manifest() {
    let json = parse(RuntimeManifest {
        required_files: vec!["parakit.exe".to_string(), "crispasr.dll".to_string()],
        accelerator: Accelerator::Cpu,
        cuda: None,
        vulkan: None,
    });

    assert_eq!(json["required_files"][0], "parakit.exe");
    assert_eq!(json["accelerator"], "cpu");
    assert_eq!(json["cuda"], Value::Null);
    assert_eq!(json["vulkan"], Value::Null);
}

#[test]
fn serializes_cuda_external_dll_contract() {
    let json = parse(RuntimeManifest {
        required_files: vec!["parakit.exe".to_string(), "ggml-cuda.dll".to_string()],
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
fn serializes_multi_backend_metadata_when_both_are_present() {
    let json = parse(RuntimeManifest {
        required_files: vec!["parakit.exe".to_string()],
        accelerator: Accelerator::Cuda,
        cuda: Some(CudaManifest {
            toolkit_version: "12.9".to_string(),
            architectures: "native".to_string(),
            external_dlls: vec![
                "cudart64_12.dll".to_string(),
                "cublas64_12.dll".to_string(),
                "cublasLt64_12.dll".to_string(),
            ],
            external_dlls_bundled: true,
        }),
        vulkan: Some(VulkanManifest {
            sdk_version: "C:\\VulkanSDK\\1.4.321.1".to_string(),
            external_dlls: vec!["vulkan-1.dll".to_string()],
            external_dlls_bundled: false,
        }),
    });

    assert_eq!(json["accelerator"], "cuda");
    assert_eq!(json["cuda"]["toolkit_version"], "12.9");
    assert_eq!(json["cuda"]["architectures"], "native");
    assert_eq!(json["cuda"]["external_dlls_bundled"], true);
    assert_eq!(json["vulkan"]["sdk_version"], "C:\\VulkanSDK\\1.4.321.1");
    assert_eq!(json["vulkan"]["external_dlls"][0], "vulkan-1.dll");
}
