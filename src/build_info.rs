//! Build-time CrispASR and ggml configuration reported by `build.rs`.

macro_rules! build_value {
    ($key:literal) => {
        option_env!($key).unwrap_or("unknown")
    };
}

/// Return a concise build summary for diagnostics.
///
/// # Returns
///
/// Lines describing the bundled ggml CPU and accelerator build settings.
pub fn diagnostic_lines() -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format!(
        "type:           {}",
        option_env!("PARAKIT_BUILD_CMAKE_BUILD_TYPE").unwrap_or("external or unknown")
    ));
    lines.push(format!(
        "cpu:            native={}, variants={}, openmp={}, repack={}, blas={}",
        build_value!("PARAKIT_BUILD_GGML_NATIVE"),
        build_value!("PARAKIT_BUILD_GGML_CPU_ALL_VARIANTS"),
        build_value!("PARAKIT_BUILD_GGML_OPENMP"),
        build_value!("PARAKIT_BUILD_GGML_CPU_REPACK"),
        blas_label()
    ));
    lines.push(format!(
        "backend dl:     {}",
        build_value!("PARAKIT_BUILD_GGML_BACKEND_DL")
    ));
    lines.push(format!(
        "cpu flags:      {}",
        option_env!("PARAKIT_BUILD_CPU_FLAGS").unwrap_or("unknown")
    ));
    if let Some(flags) = option_env!("PARAKIT_BUILD_CMAKE_CXX_FLAGS_RELEASE") {
        if !flags.is_empty() {
            lines.push(format!("c++ release:    {flags}"));
        }
    }
    lines.push(format!(
        "accelerators:   cuda={}, vulkan={}, metal={}",
        build_value!("PARAKIT_BUILD_GGML_CUDA"),
        build_value!("PARAKIT_BUILD_GGML_VULKAN"),
        build_value!("PARAKIT_BUILD_GGML_METAL")
    ));

    if let Some(arch) = option_env!("PARAKIT_BUILD_CMAKE_CUDA_ARCHITECTURES") {
        if !arch.is_empty() {
            lines.push(format!("cuda arch:      {arch}"));
        }
    }
    if let Some(request) = non_empty_env(option_env!("PARAKIT_BUILD_CUDA_ARCHS_REQUEST")) {
        lines.push(format!("cuda request:   {request}"));
    }
    if let Some(version) = non_empty_env(option_env!("PARAKIT_BUILD_CUDA_TOOLKIT_VERSION")) {
        lines.push(format!("cuda toolkit:   {version}"));
    }
    if let Some(compiler) = non_empty_env(option_env!("PARAKIT_BUILD_CMAKE_CUDA_COMPILER")) {
        lines.push(format!("cuda compiler:  {compiler}"));
    }
    if let Some(sdk) = non_empty_env(option_env!("PARAKIT_BUILD_VULKAN_SDK")) {
        lines.push(format!("vulkan sdk:     {sdk}"));
    }

    lines
}

/// Return whether this build has a compiled GPU accelerator.
pub fn accelerator_enabled() -> bool {
    cuda_enabled() || vulkan_enabled() || metal_enabled()
}

/// Return whether this build enabled ggml CUDA.
pub fn cuda_enabled() -> bool {
    build_value!("PARAKIT_BUILD_GGML_CUDA") == "ON"
}

/// Return whether this build enabled ggml Vulkan.
pub fn vulkan_enabled() -> bool {
    build_value!("PARAKIT_BUILD_GGML_VULKAN") == "ON"
}

/// Return whether this build enabled ggml Metal.
pub fn metal_enabled() -> bool {
    build_value!("PARAKIT_BUILD_GGML_METAL") == "ON"
}

fn non_empty_env(value: Option<&'static str>) -> Option<&'static str> {
    value.filter(|value| !value.is_empty() && *value != "unknown")
}

fn blas_label() -> String {
    let enabled = build_value!("PARAKIT_BUILD_GGML_BLAS");
    let selected = option_env!("PARAKIT_BUILD_BLAS_SELECTED").unwrap_or("unknown");
    let vendor = option_env!("PARAKIT_BUILD_GGML_BLAS_VENDOR").unwrap_or("");
    let cohere_mkl = build_value!("PARAKIT_BUILD_COHERE_MKL");
    if enabled == "OFF" || selected == "off" {
        return "OFF".to_string();
    }
    if vendor.is_empty() {
        format!("{enabled} ({selected})")
    } else if cohere_mkl == "ON" {
        format!("{enabled} ({selected}, vendor={vendor}, cohere_mkl=ON)")
    } else {
        format!("{enabled} ({selected}, vendor={vendor})")
    }
}
