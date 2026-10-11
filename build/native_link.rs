//! Native linker directives supplied by Parakit instead of crispasr-sys.

use std::env;
use std::path::Path;

/// Link the platform C++ runtime used by the CrispASR native library.
pub(crate) fn emit_cpp_runtime() {
    match env::var("CARGO_CFG_TARGET_OS").unwrap_or_default().as_str() {
        "linux" => println!("cargo:rustc-link-lib=dylib=stdc++"),
        "macos" => println!("cargo:rustc-link-lib=dylib=c++"),
        _ => {}
    }
}

/// Emit the explicit external library directory followed by system prefixes.
///
/// # Arguments
///
/// * `explicit_dir` - Optional `CRISPASR_LIB_DIR` override to search first.
pub(crate) fn emit_external_search_paths(explicit_dir: Option<&Path>) {
    if let Some(dir) = explicit_dir {
        println!("cargo:rustc-link-search=native={}", dir.display());
    }
    for dir in [
        "/opt/homebrew/lib",
        "/usr/local/lib",
        "/usr/lib",
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
    ] {
        if Path::new(dir).is_dir() {
            println!("cargo:rustc-link-search=native={dir}");
        }
    }
}

/// Link ggml directly when its device registry is used by the bundled build.
///
/// # Arguments
///
/// * `bundled` - Whether bundled device enumeration is compiled.
pub(crate) fn emit_direct_ggml_link_if_bundled(bundled: bool) {
    if bundled {
        println!("cargo:rustc-link-lib=dylib=ggml");
    }
}

/// Configure the raw-PCM runtime without optional file-decoder dependencies.
///
/// # Arguments
///
/// * `cfg` - CrispASR native build configuration.
/// * `blas_enabled` - Whether Parakit's selected BLAS policy enables BLAS.
pub(crate) fn configure_pcm_runtime(cfg: &mut cmake::Config, blas_enabled: bool) {
    cfg.define("CRISPASR_OPUS", "OFF")
        .define("CRISPASR_AMR", "OFF")
        .define("CRISPASR_MEL_BLAS", if blas_enabled { "ON" } else { "OFF" });
}

/// Compile the device adapter against the selected native ggml headers.
///
/// # Arguments
///
/// * `src_dir` - CrispASR source tree selected by the bundled build.
pub(crate) fn build_device_shim(src_dir: &Path) {
    println!("cargo:rerun-if-changed=src/ggml_device_shim.cpp");
    for header in [
        "include/ggml.h",
        "include/ggml-backend.h",
        "src/ggml-backend-impl.h",
    ] {
        println!(
            "cargo:rerun-if-changed={}",
            src_dir.join("ggml").join(header).display()
        );
    }
    cc::Build::new()
        .cpp(true)
        .std("c++17")
        // CrispASR is shared; Windows declarations use its import library.
        .define("GGML_SHARED", None)
        .include(src_dir.join("ggml/include"))
        .include(src_dir.join("ggml/src"))
        .file("src/ggml_device_shim.cpp")
        .compile("parakit_ggml_device");
}
