//! Windows runtime-manifest serialization used by `build.rs`.
//!
//! This lives outside `build.rs` so integration tests can exercise the JSON
//! shape without invoking the native CMake build.

use serde_json::{json, Value};

/// Accelerator flavor recorded in the Windows runtime manifest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Accelerator {
    Cpu,
    Cuda,
    Vulkan,
}

impl Accelerator {
    fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Cuda => "cuda",
            Self::Vulkan => "vulkan",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// CUDA fields serialized into the Windows runtime manifest.
pub(crate) struct CudaManifest {
    pub(crate) toolkit_version: String,
    pub(crate) architectures: String,
    pub(crate) external_dlls: Vec<String>,
    pub(crate) external_dlls_bundled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Vulkan fields serialized into the Windows runtime manifest.
pub(crate) struct VulkanManifest {
    pub(crate) sdk_version: String,
    pub(crate) external_dlls: Vec<String>,
    pub(crate) external_dlls_bundled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// Complete Windows runtime manifest model.
pub(crate) struct RuntimeManifest {
    pub(crate) required_files: Vec<String>,
    pub(crate) accelerator: Accelerator,
    pub(crate) cuda: Option<CudaManifest>,
    pub(crate) vulkan: Option<VulkanManifest>,
}

impl RuntimeManifest {
    /// Serialize the manifest as stable JSON.
    ///
    /// # Returns
    ///
    /// A pretty-printed JSON document terminated by a newline.
    pub(crate) fn to_json(&self) -> String {
        let manifest = json!({
            "required_files": &self.required_files,
            "accelerator": self.accelerator.as_str(),
            "cuda": self.cuda.as_ref().map(cuda_manifest_value),
            "vulkan": self.vulkan.as_ref().map(vulkan_manifest_value),
        });
        format!(
            "{}\n",
            serde_json::to_string_pretty(&manifest).expect("runtime manifest JSON serialization")
        )
    }
}

/// Find DLLs owned by a previous runtime manifest that are absent from the
/// current bundle.
///
/// # Arguments
///
/// * `previous_json` - Previously staged runtime manifest JSON.
/// * `current_dlls` - DLL file names produced by the current build.
///
/// # Returns
///
/// Flat DLL file names that should be removed before staging the current
/// bundle, or an empty vector when the previous manifest cannot be read.
pub(crate) fn stale_runtime_dlls(previous_json: &str, current_dlls: &[String]) -> Vec<String> {
    let Ok(manifest) = serde_json::from_str::<Value>(previous_json) else {
        return Vec::new();
    };
    let Some(required_files) = manifest.get("required_files").and_then(Value::as_array) else {
        return Vec::new();
    };

    required_files
        .iter()
        .filter_map(Value::as_str)
        .filter(|name| {
            name.to_ascii_lowercase().ends_with(".dll")
                && !name.contains('/')
                && !name.contains('\\')
                && !current_dlls
                    .iter()
                    .any(|current| current.eq_ignore_ascii_case(name))
        })
        .map(str::to_string)
        .collect()
}

fn cuda_manifest_value(cuda: &CudaManifest) -> Value {
    json!({
        "toolkit_version": &cuda.toolkit_version,
        "architectures": &cuda.architectures,
        "external_dlls": &cuda.external_dlls,
        "external_dlls_bundled": cuda.external_dlls_bundled,
    })
}

fn vulkan_manifest_value(vulkan: &VulkanManifest) -> Value {
    json!({
        "sdk_version": &vulkan.sdk_version,
        "external_dlls": &vulkan.external_dlls,
        "external_dlls_bundled": vulkan.external_dlls_bundled,
    })
}
