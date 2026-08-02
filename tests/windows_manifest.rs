//! Integration coverage for Windows runtime-manifest serialization.

#[path = "../build/windows_manifest.rs"]
mod windows_manifest;

use serde_json::{json, Value};

use windows_manifest::{
    stale_runtime_dlls, Accelerator, CudaManifest, RuntimeManifest, VulkanManifest,
};

fn parse(manifest: RuntimeManifest) -> Value {
    serde_json::from_str(&manifest.to_json()).expect("manifest should serialize valid JSON")
}

/// Expected `"accelerator"` JSON value for each backend flavor.
///
/// This match has no wildcard arm: adding an `Accelerator` variant without
/// stating its serialized label here fails compilation.
fn expected_accelerator_label(accelerator: Accelerator) -> &'static str {
    match accelerator {
        Accelerator::Cpu => "cpu",
        Accelerator::Cuda => "cuda",
        Accelerator::Vulkan => "vulkan",
    }
}

/// One serialized-manifest scenario: the manifest built from the real
/// production structs, and the JSON fields (as RFC 6901 pointers) its
/// serialized form must contain.
struct Case {
    name: &'static str,
    manifest: RuntimeManifest,
    expect_json_fields: Vec<(&'static str, Value)>,
}

#[test]
fn serializes_runtime_manifest_cases() {
    let cases = vec![
        Case {
            name: "cpu-manifest",
            manifest: RuntimeManifest {
                required_files: vec!["parakit.exe".to_string(), "crispasr.dll".to_string()],
                accelerator: Accelerator::Cpu,
                cuda: None,
                vulkan: None,
            },
            expect_json_fields: vec![
                ("/required_files/0", json!("parakit.exe")),
                ("/cuda", Value::Null),
                ("/vulkan", Value::Null),
            ],
        },
        Case {
            name: "cuda-external-dll-contract",
            manifest: RuntimeManifest {
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
            },
            expect_json_fields: vec![
                ("/cuda/toolkit_version", json!("13.2")),
                ("/cuda/architectures", json!("89-real")),
                ("/cuda/external_dlls/0", json!("cudart64_13.dll")),
                ("/cuda/external_dlls/1", json!("cublas64_13.dll")),
                ("/cuda/external_dlls/2", json!("cublasLt64_13.dll")),
                ("/cuda/external_dlls_bundled", json!(false)),
                ("/vulkan", Value::Null),
            ],
        },
        Case {
            name: "vulkan-system-loader-contract",
            manifest: RuntimeManifest {
                required_files: vec!["parakit.exe".to_string(), "ggml-vulkan.dll".to_string()],
                accelerator: Accelerator::Vulkan,
                cuda: None,
                vulkan: Some(VulkanManifest {
                    sdk_version: "1.4.321.1".to_string(),
                    external_dlls: vec!["vulkan-1.dll".to_string()],
                    external_dlls_bundled: false,
                }),
            },
            expect_json_fields: vec![
                ("/cuda", Value::Null),
                ("/vulkan/sdk_version", json!("1.4.321.1")),
                ("/vulkan/external_dlls/0", json!("vulkan-1.dll")),
                ("/vulkan/external_dlls_bundled", json!(false)),
            ],
        },
    ];

    let mut failures = Vec::new();
    for case in cases {
        let accelerator = case.manifest.accelerator;
        let json = parse(case.manifest);

        let expected_label = expected_accelerator_label(accelerator);
        if json["accelerator"] != expected_label {
            failures.push(format!(
                "{}: accelerator: expected {expected_label:?}, got {:?}",
                case.name, json["accelerator"]
            ));
        }
        for (pointer, expected) in &case.expect_json_fields {
            if json.pointer(pointer) != Some(expected) {
                failures.push(format!(
                    "{}: {pointer}: expected {expected}, got {:?}",
                    case.name,
                    json.pointer(pointer)
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} case(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn identifies_only_previous_bundle_dlls_missing_from_current_build() {
    let previous = RuntimeManifest {
        required_files: vec![
            "parakit.exe".to_string(),
            "crispasr.dll".to_string(),
            "ggml-cuda.dll".to_string(),
            "cublas64_13.dll".to_string(),
        ],
        accelerator: Accelerator::Cuda,
        cuda: None,
        vulkan: None,
    }
    .to_json();
    let current = vec!["CRISPASR.DLL".to_string(), "ggml-vulkan.dll".to_string()];

    assert_eq!(
        stale_runtime_dlls(&previous, &current),
        vec!["ggml-cuda.dll", "cublas64_13.dll"]
    );
}
