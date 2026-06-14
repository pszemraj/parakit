//! Windows runtime-manifest serialization used by `build.rs`.
//!
//! This lives outside `build.rs` so integration tests can exercise the JSON
//! shape without invoking the native CMake build.

use serde_json::{json, Value};

/// File name for the Windows runtime manifest colocated with `parakit.exe`.
pub(crate) const WINDOWS_RUNTIME_MANIFEST: &str = "parakit-runtime-manifest.json";

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
/// BLAS fields serialized into the Windows runtime manifest.
pub(crate) struct BlasManifest {
    pub(crate) requested: String,
    pub(crate) selected: String,
    pub(crate) openblas_root: Option<String>,
    pub(crate) openblas_include_dir: Option<String>,
    pub(crate) openblas_import_lib: Option<String>,
    pub(crate) openblas_runtime_dlls: Vec<String>,
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
    pub(crate) runtime_dlls: Vec<String>,
    pub(crate) blas: BlasManifest,
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
            "runtime_dlls": &self.runtime_dlls,
            "blas": {
                "requested": &self.blas.requested,
                "selected": &self.blas.selected,
            },
            "openblas_root": &self.blas.openblas_root,
            "openblas_include_dir": &self.blas.openblas_include_dir,
            "openblas_import_lib": &self.blas.openblas_import_lib,
            "openblas_runtime_dlls": &self.blas.openblas_runtime_dlls,
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

/// Derive cuBLAS runtime DLL names from a CUDA Toolkit version string.
///
/// # Returns
///
/// The expected `cublas64_<major>.dll` and `cublasLt64_<major>.dll` names,
/// or an empty vector when no numeric major version can be parsed.
pub(crate) fn cuda_external_dll_names(toolkit_version: &str) -> Vec<String> {
    let Some(major) = toolkit_version
        .split(|ch: char| !ch.is_ascii_digit())
        .find(|part| !part.is_empty())
    else {
        return Vec::new();
    };
    vec![
        format!("cublas64_{major}.dll"),
        format!("cublasLt64_{major}.dll"),
    ]
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
