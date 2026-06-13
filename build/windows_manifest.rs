//! Windows runtime-manifest serialization used by `build.rs`.
//!
//! This lives outside `build.rs` so integration tests can exercise the JSON
//! shape without invoking the native CMake build.

pub(crate) const WINDOWS_RUNTIME_MANIFEST: &str = "parakit-runtime-manifest.json";

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
pub(crate) struct BlasManifest {
    pub(crate) requested: String,
    pub(crate) selected: String,
    pub(crate) openblas_root: Option<String>,
    pub(crate) openblas_include_dir: Option<String>,
    pub(crate) openblas_import_lib: Option<String>,
    pub(crate) openblas_runtime_dlls: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CudaManifest {
    pub(crate) toolkit_version: String,
    pub(crate) architectures: String,
    pub(crate) external_dlls: Vec<String>,
    pub(crate) external_dlls_bundled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct VulkanManifest {
    pub(crate) sdk_version: String,
    pub(crate) external_dlls: Vec<String>,
    pub(crate) external_dlls_bundled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RuntimeManifest {
    pub(crate) required_files: Vec<String>,
    pub(crate) runtime_dlls: Vec<String>,
    pub(crate) blas: BlasManifest,
    pub(crate) accelerator: Accelerator,
    pub(crate) cuda: Option<CudaManifest>,
    pub(crate) vulkan: Option<VulkanManifest>,
}

impl RuntimeManifest {
    pub(crate) fn to_json(&self) -> String {
        format!(
            "{{\n  \"required_files\": {},\n  \"runtime_dlls\": {},\n  \"blas\": {{\n    \"requested\": {},\n    \"selected\": {}\n  }},\n  \"openblas_root\": {},\n  \"openblas_include_dir\": {},\n  \"openblas_import_lib\": {},\n  \"openblas_runtime_dlls\": {},\n  \"accelerator\": {},\n  \"cuda\": {},\n  \"vulkan\": {}\n}}\n",
            json_array(&self.required_files),
            json_array(&self.runtime_dlls),
            json_string(&self.blas.requested),
            json_string(&self.blas.selected),
            json_nullable_string(self.blas.openblas_root.as_deref()),
            json_nullable_string(self.blas.openblas_include_dir.as_deref()),
            json_nullable_string(self.blas.openblas_import_lib.as_deref()),
            json_array(&self.blas.openblas_runtime_dlls),
            json_string(self.accelerator.as_str()),
            self.cuda
                .as_ref()
                .map(cuda_manifest_json)
                .unwrap_or_else(|| "null".to_string()),
            self.vulkan
                .as_ref()
                .map(vulkan_manifest_json)
                .unwrap_or_else(|| "null".to_string()),
        )
    }
}

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

fn cuda_manifest_json(cuda: &CudaManifest) -> String {
    format!(
        "{{\n    \"toolkit_version\": {},\n    \"architectures\": {},\n    \"external_dlls\": {},\n    \"external_dlls_bundled\": {}\n  }}",
        json_string(&cuda.toolkit_version),
        json_string(&cuda.architectures),
        json_array(&cuda.external_dlls),
        json_bool(cuda.external_dlls_bundled),
    )
}

fn vulkan_manifest_json(vulkan: &VulkanManifest) -> String {
    format!(
        "{{\n    \"sdk_version\": {},\n    \"external_dlls\": {},\n    \"external_dlls_bundled\": {}\n  }}",
        json_string(&vulkan.sdk_version),
        json_array(&vulkan.external_dlls),
        json_bool(vulkan.external_dlls_bundled),
    )
}

fn json_array(values: &[String]) -> String {
    let escaped = values
        .iter()
        .map(|value| json_string(value))
        .collect::<Vec<_>>();
    format!("[{}]", escaped.join(", "))
}

fn json_nullable_string(value: Option<&str>) -> String {
    value.map(json_string).unwrap_or_else(|| "null".to_string())
}

fn json_bool(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}
