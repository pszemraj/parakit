//! Build-time CrispASR and ggml configuration reported by `build.rs`.

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
        "cpu:            native={}, openmp={}, repack={}, blas={}",
        ggml_native(),
        ggml_openmp(),
        ggml_cpu_repack(),
        blas_label()
    ));
    lines.push(format!(
        "cpu flags:      {}",
        option_env!("PARAKIT_BUILD_CPU_FLAGS").unwrap_or("unknown")
    ));
    lines.push(format!(
        "accelerators:   cuda={}, vulkan={}, metal={}",
        ggml_cuda(),
        ggml_vulkan(),
        ggml_metal()
    ));

    if let Some(arch) = option_env!("PARAKIT_BUILD_CMAKE_CUDA_ARCHITECTURES") {
        if !arch.is_empty() {
            lines.push(format!("cuda arch:      {arch}"));
        }
    }

    lines
}

fn blas_label() -> String {
    let enabled = ggml_blas();
    let selected = option_env!("PARAKIT_BUILD_BLAS_SELECTED").unwrap_or("unknown");
    let vendor = option_env!("PARAKIT_BUILD_GGML_BLAS_VENDOR").unwrap_or("");
    let cohere_mkl = cohere_mkl();
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

fn ggml_native() -> &'static str {
    option_env!("PARAKIT_BUILD_GGML_NATIVE").unwrap_or("unknown")
}

fn ggml_openmp() -> &'static str {
    option_env!("PARAKIT_BUILD_GGML_OPENMP").unwrap_or("unknown")
}

fn ggml_cpu_repack() -> &'static str {
    option_env!("PARAKIT_BUILD_GGML_CPU_REPACK").unwrap_or("unknown")
}

fn ggml_blas() -> &'static str {
    option_env!("PARAKIT_BUILD_GGML_BLAS").unwrap_or("unknown")
}

fn cohere_mkl() -> &'static str {
    option_env!("PARAKIT_BUILD_COHERE_MKL").unwrap_or("unknown")
}

fn ggml_cuda() -> &'static str {
    option_env!("PARAKIT_BUILD_GGML_CUDA").unwrap_or("unknown")
}

fn ggml_vulkan() -> &'static str {
    option_env!("PARAKIT_BUILD_GGML_VULKAN").unwrap_or("unknown")
}

fn ggml_metal() -> &'static str {
    option_env!("PARAKIT_BUILD_GGML_METAL").unwrap_or("unknown")
}
