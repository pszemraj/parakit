//! Builds (or locates) the CrispASR C library and tells the linker where it is.
//!
//! Default behavior (with the `bundled` feature on, which is the default):
//!   - Vendored source is at `vendor/CrispASR/`. Cargo must see that submodule
//!     before dependency resolution because the `crispasr` Rust crate is a path
//!     dependency.
//!   - Configure & build via the `cmake` crate. Output is cached in
//!     `OUT_DIR` so subsequent `cargo build` invocations are incremental.
//!   - Backend selection (`cuda` / `metal` / `vulkan`) is driven by parakit's
//!     own cargo features, not by `crispasr-sys` (which is a pure shim and
//!     doesn't compile anything).
//!   - Emits a `rustc-link-search` so `crispasr-sys`'s `link-lib=crispasr`
//!     resolves to our just-built library.
//!   - Emits an rpath on Unix so the binary finds the dylib at runtime
//!     without needing LD_LIBRARY_PATH.
//!
//! Escape hatches (any one of these skips the bundled build):
//!   - `--no-default-features`              : the user takes responsibility for
//!     providing libcrispasr; add `--features daemon` when building the daemon.
//!   - `CRISPASR_LIB_DIR=/path/to/libdir`   : link-search path override.
//!   - `CRISPASR_SRC_DIR=/path/to/source`   : use this checkout instead of the vendored source.
//!
//! These still require Cargo to load the `crispasr` Rust path dependency unless
//! the manifest is changed, so a missing submodule must be fixed before Cargo
//! can start this script.

use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "build/openblas_roots.rs"]
mod openblas_roots;
use openblas_roots::{
    configured_openblas_roots, conventional_openblas_roots, env_path, OPENBLAS_DISCOVERY_ENV_VARS,
};

#[path = "build/unix_openblas.rs"]
mod unix_openblas;
use unix_openblas::{find_unix_openblas, UnixOpenBlas};

#[path = "build/windows_openblas.rs"]
mod windows_openblas;
use windows_openblas::{find_windows_openblas, WindowsOpenBlas, WindowsOpenBlasImportKind};

#[path = "build/windows_artifacts.rs"]
mod windows_artifacts;
#[path = "build/windows_cuda.rs"]
mod windows_cuda;
#[path = "build/windows_manifest.rs"]
mod windows_manifest;

fn main() {
    println!("cargo:rerun-if-env-changed=CRISPASR_LIB_DIR");
    println!("cargo:rerun-if-env-changed=CRISPASR_SRC_DIR");
    println!("cargo:rerun-if-env-changed=PARAKIT_BLAS");
    for name in OPENBLAS_DISCOVERY_ENV_VARS {
        println!("cargo:rerun-if-env-changed={name}");
    }
    println!("cargo:rerun-if-env-changed=PARAKIT_CUDA_ARCHS");
    println!("cargo:rerun-if-env-changed=PARAKIT_BUNDLE_CUDA_DLLS");
    println!("cargo:rerun-if-env-changed=CUDA_PATH");
    println!("cargo:rerun-if-env-changed=VULKAN_SDK");
    println!("cargo:rerun-if-env-changed=BLAS_INCLUDE_DIRS");
    println!("cargo:rerun-if-env-changed=BLAS_LIBRARIES");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build/openblas_roots.rs");
    println!("cargo:rerun-if-changed=build/unix_openblas.rs");
    println!("cargo:rerun-if-changed=build/windows_openblas.rs");
    println!("cargo:rerun-if-changed=build/windows_manifest.rs");
    println!("cargo:rerun-if-changed=build/windows_cuda.rs");
    println!("cargo:rerun-if-changed=build/windows_artifacts.rs");

    let bundled = cargo_feature("bundled");
    build_alsa_silencer(cargo_feature("daemon"));

    // 1. Honor an explicit lib-dir override regardless of feature flags.
    //    crispasr-sys reads CRISPASR_LIB_DIR too — we add to its search path
    //    here so `cargo build` works without the user re-exporting the var.
    if let Ok(dir) = env::var("CRISPASR_LIB_DIR") {
        println!("cargo:rustc-link-search=native={dir}");
        emit_rpath(Path::new(&dir));
        emit_direct_ggml_link_if_bundled(bundled);
        return;
    }

    // 2. If the user disabled the `bundled` feature, do nothing.
    //    crispasr-sys's own build.rs will probe /usr/local/lib, /opt/homebrew/lib, etc.
    if !bundled {
        return;
    }

    // 3. Locate the CrispASR source tree.
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let src_dir = locate_source(&manifest_dir);
    println!(
        "cargo:rerun-if-changed={}/CMakeLists.txt",
        src_dir.display()
    );
    println!(
        "cargo:rerun-if-changed={}/src/parakeet.cpp",
        src_dir.display()
    );

    // 4. Run cmake. The `cmake` crate handles incremental builds, MSVC
    //    detection on Windows, generator selection, parallelism, and
    //    install-step plumbing.
    let build_crispasr_examples = !target_is_windows();
    let mut cfg = cmake::Config::new(&src_dir);
    cfg.profile("Release")
        .define("BUILD_SHARED_LIBS", "ON")
        // parakit is normally built from source for the machine it runs on.
        // Make the CPU policy explicit instead of relying on ggml defaults.
        .define("GGML_NATIVE", "ON")
        .define("GGML_CPU_ALL_VARIANTS", "OFF")
        .define("GGML_OPENMP", "ON")
        .define("GGML_CPU_REPACK", "ON")
        .define("GGML_BACKEND_DL", "OFF")
        // Skip tests. On Unix we build examples because CrispASR's quantizer
        // lives there. On Windows, the pinned CrispASR examples tree also
        // builds the server target, which currently fails under MSVC before
        // parakit can link; hosted Q8 remains the Windows CPU-first path.
        .define("CRISPASR_BUILD_TESTS", "OFF")
        .define(
            "CRISPASR_BUILD_EXAMPLES",
            cmake_bool(build_crispasr_examples),
        )
        .define("GGML_BUILD_TESTS", "OFF")
        .define("GGML_BUILD_EXAMPLES", "OFF")
        // Bake an `$ORIGIN` (Linux/BSD) or `@loader_path` (macOS) rpath into
        // the installed shared libraries so each library finds its siblings
        // (libcrispasr.so → libggml.so → libggml-cpu.so) without LD_LIBRARY_PATH.
        // Without this, libcrispasr.so's transitive deps (libggml*.so) fail to
        // resolve at load time even when the binary's own rpath points at
        // the right directory — Linux's DT_RUNPATH doesn't apply transitively.
        .define("CMAKE_INSTALL_RPATH", install_rpath_token())
        .define("CMAKE_BUILD_WITH_INSTALL_RPATH", "ON")
        .define("CMAKE_INSTALL_RPATH_USE_LINK_PATH", "ON");

    configure_windows_msvc_release(&mut cfg);

    let accelerators = AcceleratorConfig::from_env();

    let blas = configure_blas(&mut cfg);
    cfg.define("GGML_CUDA", cmake_bool(accelerators.cuda_enabled));
    if accelerators.cuda_enabled {
        cfg.define("GGML_CUDA_NCCL", "OFF");
        if let Some(archs) = accelerators.cuda_archs_request.as_ref() {
            cfg.define("CMAKE_CUDA_ARCHITECTURES", archs);
        }
    }
    cfg.define("GGML_VULKAN", cmake_bool(accelerators.vulkan_enabled));
    if accelerators.metal_enabled {
        if target_is_apple() {
            cfg.define("GGML_METAL", "ON");
            cfg.define("GGML_METAL_EMBED_LIBRARY", "ON");
        } else if accelerators.cuda_enabled || accelerators.vulkan_enabled {
            cfg.define("GGML_METAL", "OFF");
            println!(
                "cargo:warning=ignoring unsupported metal feature on non-Apple target during multi-backend build"
            );
        } else {
            panic!("the metal feature is only supported on Apple targets");
        }
    } else {
        cfg.define("GGML_METAL", "OFF");
    }

    let install_dir = cfg.build();
    emit_build_report(&install_dir, &accelerators);

    // 5. CrispASR v0.6.6 installs `libcrispasr` as the umbrella library.
    //    `crispasr-sys` links that exact name, so fail clearly if the pinned
    //    submodule stops producing it.
    let lib_dir = install_dir.join("lib");
    let lib_dir_alt = install_dir.join("lib64");
    let final_lib_dir = if lib_dir.is_dir() {
        lib_dir
    } else if lib_dir_alt.is_dir() {
        lib_dir_alt
    } else if target_is_windows() {
        std::fs::create_dir_all(&lib_dir).unwrap_or_else(|err| {
            panic!(
                "failed to create Windows import-library dir {}: {err}",
                lib_dir.display()
            )
        });
        lib_dir
    } else {
        panic!(
            "expected install dir to contain lib/ or lib64/, got {}",
            install_dir.display()
        );
    };

    let bin_dir = install_dir.join("bin");
    if target_is_windows() {
        windows_artifacts::prepare_windows_artifacts(
            &install_dir,
            &final_lib_dir,
            &bin_dir,
            &blas,
            &accelerators,
        );
    } else {
        assert_crispasr_library_exists(&final_lib_dir);
        assert_apple_metal_library(&final_lib_dir, accelerators.metal_enabled);
    }

    println!("cargo:rustc-link-search=native={}", final_lib_dir.display());
    emit_direct_ggml_link_if_bundled(bundled);

    // Windows DLLs land in bin/, not lib/. Add it for completeness.
    if bin_dir.is_dir() {
        println!("cargo:rustc-link-search=native={}", bin_dir.display());
    }

    let quantize_bin = bin_dir.join(exe_name("crispasr-quantize"));
    if quantize_bin.is_file() {
        println!(
            "cargo:rustc-env=CRISPASR_QUANTIZE_BIN={}",
            quantize_bin.display()
        );
    } else if !target_is_windows() {
        println!(
            "cargo:warning=crispasr-quantize was not installed at {}; source rebuilds will look on PATH",
            quantize_bin.display()
        );
    }

    // 6. Bake the lib path into the binary's rpath so we don't need
    //    LD_LIBRARY_PATH / DYLD_FALLBACK_LIBRARY_PATH at runtime.
    //    No-op on Windows; DLLs are copied into the profile dir above.
    emit_rpath(&final_lib_dir);

    // 7. Re-export the install path. Useful for `cargo run` on macOS
    //    (where DYLD_LIBRARY_PATH is hostile) and for downstream tooling
    //    that wants to copy the dylib into a release artifact.
    println!(
        "cargo:rustc-env=CRISPASR_INSTALL_DIR={}",
        install_dir.display()
    );
}

fn build_alsa_silencer(daemon_enabled: bool) {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") || !daemon_enabled {
        return;
    }

    let shim = "src/daemon/audio/alsa_silence.c";
    println!("cargo:rerun-if-changed={shim}");
    cc::Build::new()
        .file(shim)
        .warnings(true)
        .compile("parakit_alsa_silence");
    println!("cargo:rustc-link-lib=asound");
}

fn emit_build_report(install_dir: &Path, accelerators: &AcceleratorConfig) {
    let build_dir = install_dir.join("build");
    let cache_path = build_dir.join("CMakeCache.txt");
    let cache = read_cmake_cache(&cache_path);
    let cpu_flags = read_cpu_flags(&build_dir);

    emit_env_from_cache(&cache, "CMAKE_BUILD_TYPE", "PARAKIT_BUILD_CMAKE_BUILD_TYPE");
    for key in [
        "GGML_NATIVE",
        "GGML_CPU_ALL_VARIANTS",
        "GGML_OPENMP",
        "GGML_CPU_REPACK",
        "GGML_BACKEND_DL",
        "GGML_BLAS",
        "GGML_BLAS_VENDOR",
        "COHERE_MKL",
        "GGML_CUDA",
        "GGML_VULKAN",
        "GGML_METAL",
        "GGML_METAL_EMBED_LIBRARY",
        "CMAKE_CUDA_ARCHITECTURES",
        "CMAKE_CUDA_COMPILER",
        "CMAKE_C_FLAGS_RELEASE",
        "CMAKE_CXX_FLAGS_RELEASE",
        "CMAKE_ASM_FLAGS_RELEASE",
        "GGML_CCACHE",
    ] {
        let env_key = format!("PARAKIT_BUILD_{key}");
        emit_env_from_cache(&cache, key, &env_key);
    }

    if let Some(flags) = cpu_flags {
        println!("cargo:rustc-env=PARAKIT_BUILD_CPU_FLAGS={flags}");
    } else if target_is_windows() {
        println!("cargo:rustc-env=PARAKIT_BUILD_CPU_FLAGS=unavailable on Windows CMake generator");
    } else {
        println!(
            "cargo:warning=could not read ggml CPU flags from {}",
            build_dir.display()
        );
    }

    if accelerators.cuda_enabled {
        let cuda_archs_request = accelerators
            .cuda_archs_request
            .as_deref()
            .unwrap_or("native");
        println!("cargo:rustc-env=PARAKIT_BUILD_CUDA_ARCHS_REQUEST={cuda_archs_request}");

        match accelerators.cuda_toolkit_version.as_deref() {
            Some(version) => {
                println!("cargo:rustc-env=PARAKIT_BUILD_CUDA_TOOLKIT_VERSION={version}");
            }
            None => {
                println!(
                    "cargo:warning=could not determine CUDA Toolkit version from nvcc --version"
                );
                println!("cargo:rustc-env=PARAKIT_BUILD_CUDA_TOOLKIT_VERSION=unknown");
            }
        }
    }

    if accelerators.vulkan_enabled {
        let sdk = accelerators
            .vulkan_sdk_version
            .as_deref()
            .unwrap_or("unknown");
        println!("cargo:rustc-env=PARAKIT_BUILD_VULKAN_SDK={sdk}");
    }
}

fn configure_windows_msvc_release(cfg: &mut cmake::Config) {
    if !target_is_windows() || env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }

    // The cmake crate writes Visual Studio config flags itself so it can
    // communicate /MD vs /MT. Without setting Release flags explicitly, the
    // generated MSVC Release project can lose /O2 and build ggml unoptimized.
    const MSVC_RELEASE_FLAGS: &str = "/O2 /Ob2 /DNDEBUG /MD /utf-8 /W0";

    cfg.define("CMAKE_C_FLAGS_RELEASE", MSVC_RELEASE_FLAGS)
        .define("CMAKE_CXX_FLAGS_RELEASE", MSVC_RELEASE_FLAGS)
        .define("CMAKE_ASM_FLAGS_RELEASE", MSVC_RELEASE_FLAGS)
        // ggml enables ccache when it finds it. On Windows that has produced
        // permission failures in normal developer shells, so keep MSVC builds
        // direct and deterministic.
        .define("GGML_CCACHE", "OFF");
}

fn configure_blas(cfg: &mut cmake::Config) -> BlasConfig {
    let blas = BlasConfig::from_env();
    cfg.define("GGML_BLAS", cmake_bool(blas.enabled));
    cfg.define("COHERE_MKL", cmake_bool(blas.cohere_mkl));
    if let Some(vendor) = blas.vendor {
        cfg.define("GGML_BLAS_VENDOR", vendor);
        if blas.cohere_mkl {
            cfg.define("BLA_VENDOR", vendor);
        }
    }

    println!(
        "cargo:rustc-env=PARAKIT_BUILD_BLAS_REQUEST={}",
        blas.requested
    );
    println!(
        "cargo:rustc-env=PARAKIT_BUILD_BLAS_SELECTED={}",
        blas.selected
    );
    if blas.explicit {
        println!(
            "cargo:warning=parakit build: PARAKIT_BLAS={} selected {}",
            blas.requested, blas.selected
        );
    }
    configure_blas_paths(cfg, &blas);
    blas
}

struct BlasConfig {
    requested: String,
    selected: &'static str,
    enabled: bool,
    vendor: Option<&'static str>,
    cohere_mkl: bool,
    explicit: bool,
    openblas_install: Option<OpenBlasInstall>,
}

enum OpenBlasInstall {
    Windows(WindowsOpenBlas),
    Unix(UnixOpenBlas),
}

struct AcceleratorConfig {
    cuda_enabled: bool,
    metal_enabled: bool,
    vulkan_enabled: bool,
    cuda_archs_request: Option<String>,
    cuda_toolkit_version: Option<String>,
    vulkan_sdk_version: Option<String>,
}

impl AcceleratorConfig {
    fn from_env() -> Self {
        let cuda_enabled = cargo_feature("cuda");
        let vulkan_enabled = cargo_feature("vulkan");
        let cuda_archs_request = env::var("PARAKIT_CUDA_ARCHS")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        Self {
            cuda_enabled,
            metal_enabled: cargo_feature("metal"),
            vulkan_enabled,
            cuda_archs_request,
            cuda_toolkit_version: cuda_enabled.then(detect_cuda_toolkit_version).flatten(),
            vulkan_sdk_version: vulkan_enabled.then(vulkan_sdk_version).flatten(),
        }
    }
}

impl BlasConfig {
    fn from_env() -> Self {
        let raw = env::var("PARAKIT_BLAS").unwrap_or_else(|_| "auto".to_string());
        let requested = raw.trim().to_ascii_lowercase();
        let explicit = env::var("PARAKIT_BLAS").is_ok();
        match requested.as_str() {
            "" | "0" | "false" | "no" | "none" | "off" => Self::off(raw, explicit),
            "auto" => Self::auto(raw, explicit),
            "mkl" | "intel" | "intel-mkl" => Self::mkl(raw, explicit),
            "openblas" => Self::openblas(raw, explicit, detected_openblas(true, true)),
            "accelerate" | "apple" => Self::accelerate(raw, explicit),
            "1" | "true" | "yes" | "on" | "blas" | "generic" | "system" => {
                Self::generic(raw, explicit)
            }
            other => panic!(
                "unsupported PARAKIT_BLAS={other}. Use off, auto, openblas, mkl, accelerate, or generic."
            ),
        }
    }

    fn auto(raw: String, explicit: bool) -> Self {
        if target_is_apple() {
            return Self::accelerate(raw, explicit);
        }
        if target_is_windows() {
            if let Some(openblas) = windows_openblas_from_env(false) {
                return Self::openblas(raw, explicit, Some(OpenBlasInstall::Windows(openblas)));
            }
            println!(
                "cargo:warning=parakit build: PARAKIT_BLAS=auto found no bundleable Windows OpenBLAS in configured or conventional prefixes; skipping pkg-config BLAS and building without BLAS"
            );
            return Self::off(raw, explicit);
        }
        if target_is_linux() {
            if let Some(openblas) = unix_openblas_from_configured_roots(false) {
                return Self::openblas(raw, explicit, Some(OpenBlasInstall::Unix(openblas)));
            }
        }
        if pkg_config_exists("mkl-sdl") {
            return Self::mkl(raw, explicit);
        }
        if pkg_config_exists("openblas") || pkg_config_exists("openblas64") {
            return Self::openblas(raw, explicit, None);
        }
        if target_is_linux() {
            if let Some(openblas) = unix_openblas_from_conventional_roots() {
                return Self::openblas(raw, explicit, Some(OpenBlasInstall::Unix(openblas)));
            }
        }
        println!(
            "cargo:warning=parakit build: PARAKIT_BLAS=auto found no MKL/OpenBLAS pkg-config metadata or usable OpenBLAS prefix; building without BLAS"
        );
        Self::off(raw, explicit)
    }

    fn new(
        requested: String,
        selected: &'static str,
        vendor: Option<&'static str>,
        cohere_mkl: bool,
        explicit: bool,
        openblas_install: Option<OpenBlasInstall>,
    ) -> Self {
        Self {
            requested,
            selected,
            enabled: vendor.is_some(),
            vendor,
            cohere_mkl,
            explicit,
            openblas_install,
        }
    }

    fn off(requested: String, explicit: bool) -> Self {
        Self::new(requested, "off", None, false, explicit, None)
    }

    fn generic(requested: String, explicit: bool) -> Self {
        Self::new(requested, "generic", Some("Generic"), false, explicit, None)
    }

    fn openblas(
        requested: String,
        explicit: bool,
        openblas_install: Option<OpenBlasInstall>,
    ) -> Self {
        Self::new(
            requested,
            "openblas",
            Some("OpenBLAS"),
            false,
            explicit,
            openblas_install,
        )
    }

    fn mkl(requested: String, explicit: bool) -> Self {
        Self::new(requested, "mkl", Some("Intel10_64lp"), true, explicit, None)
    }

    fn accelerate(requested: String, explicit: bool) -> Self {
        if !target_is_apple() {
            panic!("PARAKIT_BLAS=accelerate is only supported on Apple targets");
        }
        Self::new(
            requested,
            "accelerate",
            Some("Apple"),
            false,
            explicit,
            None,
        )
    }
}

fn pkg_config_exists(package: &str) -> bool {
    Command::new("pkg-config")
        .args(["--exists", package])
        .status()
        .is_ok_and(|status| status.success())
}

fn configure_blas_paths(cfg: &mut cmake::Config, blas: &BlasConfig) {
    if !blas.enabled {
        return;
    }

    let manual_include_dirs = env::var("BLAS_INCLUDE_DIRS").ok();
    let manual_libraries = env::var("BLAS_LIBRARIES").ok();
    let complete_manual_override = manual_include_dirs.is_some() && manual_libraries.is_some();

    if blas.selected == "openblas" && !complete_manual_override {
        match blas.openblas_install.as_ref() {
            Some(OpenBlasInstall::Windows(openblas)) => {
                cfg.define(
                    "BLAS_LIBRARIES",
                    openblas.import_lib.to_string_lossy().as_ref(),
                );
                cfg.define(
                    "BLAS_INCLUDE_DIRS",
                    openblas.include_dir.to_string_lossy().as_ref(),
                );
                println!(
                    "cargo:warning=parakit build: using Windows OpenBLAS at {}",
                    openblas.root.display()
                );
            }
            Some(OpenBlasInstall::Unix(openblas)) => {
                cfg.define(
                    "BLAS_LIBRARIES",
                    openblas.library.to_string_lossy().as_ref(),
                );
                cfg.define(
                    "BLAS_INCLUDE_DIRS",
                    openblas.include_dir.to_string_lossy().as_ref(),
                );
                println!(
                    "cargo:warning=parakit build: using OpenBLAS at {}",
                    openblas.root.display()
                );
            }
            None => {}
        }
    }

    if let Some(include_dirs) = manual_include_dirs {
        cfg.define("BLAS_INCLUDE_DIRS", include_dirs);
    }
    if let Some(libraries) = manual_libraries {
        cfg.define("BLAS_LIBRARIES", libraries);
    }
    if complete_manual_override {
        println!(
            "cargo:warning=parakit build: using explicit BLAS_INCLUDE_DIRS and BLAS_LIBRARIES"
        );
    }
}

fn read_cmake_cache(path: &Path) -> BTreeMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        println!(
            "cargo:warning=could not read CMake cache at {}; build diagnostics will be sparse",
            path.display()
        );
        return BTreeMap::new();
    };

    let mut values = BTreeMap::new();
    for line in text.lines() {
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let Some((key_with_type, value)) = line.split_once('=') else {
            continue;
        };
        let key = key_with_type
            .split_once(':')
            .map_or(key_with_type, |(key, _)| key);
        values.insert(key.to_string(), value.to_string());
    }
    values
}

fn read_cpu_flags(build_dir: &Path) -> Option<String> {
    let flags_path = build_dir.join("ggml/src/CMakeFiles/ggml-cpu.dir/flags.make");
    let text = std::fs::read_to_string(flags_path).ok()?;
    let mut cxx_flags = None;
    let mut c_flags = None;
    for line in text.lines() {
        if let Some((key, value)) = line.split_once(" = ") {
            match key {
                "CXX_FLAGS" => cxx_flags = Some(value),
                "C_FLAGS" => c_flags = Some(value),
                _ => {}
            }
        }
    }

    let flags = cxx_flags.or(c_flags)?;
    Some(summarize_cpu_flags(flags))
}

fn detect_cuda_toolkit_version() -> Option<String> {
    let output = Command::new("nvcc").arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_nvcc_release(&text)
}

fn parse_nvcc_release(text: &str) -> Option<String> {
    for line in text.lines() {
        let Some((_, after_release)) = line.split_once("release ") else {
            continue;
        };
        let version = after_release
            .split(|ch: char| ch == ',' || ch.is_whitespace())
            .find(|part| !part.is_empty())?;
        return Some(version.to_string());
    }
    None
}

fn vulkan_sdk_version() -> Option<String> {
    let raw = env::var("VULKAN_SDK").ok()?;
    let path = PathBuf::from(&raw);
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or(Some(raw))
}

fn summarize_cpu_flags(flags: &str) -> String {
    let interesting = [
        "-O3",
        "-march=native",
        "-fopenmp",
        "-mavx512bf16",
        "-mavx512vnni",
        "-mavx512f",
        "-mavx2",
        "-mfma",
        "-mf16c",
        "-mbmi2",
        "-mavx",
        "-msse4.2",
        "/arch:AVX512",
        "/arch:AVX2",
        "/arch:AVX",
        "/arch:SSE4.2",
    ];
    let mut found = Vec::new();
    for flag in interesting {
        if flags.split_whitespace().any(|part| part == flag) {
            found.push(flag);
        }
    }
    if found.is_empty() {
        "none detected".to_string()
    } else {
        found.join(" ")
    }
}

fn emit_env_from_cache(cache: &BTreeMap<String, String>, cache_key: &str, env_key: &str) {
    if let Some(value) = cache.get(cache_key) {
        println!("cargo:rustc-env={env_key}={value}");
    }
}

fn emit_direct_ggml_link_if_bundled(bundled: bool) {
    if bundled {
        println!("cargo:rustc-link-lib=dylib=ggml");
    }
}

const fn cmake_bool(enabled: bool) -> &'static str {
    if enabled {
        "ON"
    } else {
        "OFF"
    }
}

/// Returns true if the named cargo feature is enabled.
fn cargo_feature(name: &str) -> bool {
    env::var(format!(
        "CARGO_FEATURE_{}",
        name.to_uppercase().replace('-', "_")
    ))
    .is_ok()
}

fn target_is_apple() -> bool {
    matches!(
        env::var("CARGO_CFG_TARGET_OS").unwrap_or_default().as_str(),
        "macos" | "ios"
    )
}

fn target_is_windows() -> bool {
    env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows"
}

fn target_is_linux() -> bool {
    env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "linux"
}

fn exe_name(name: &str) -> String {
    if target_is_windows() {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// Find the CrispASR source. Order:
///   1. CRISPASR_SRC_DIR (explicit override)
///   2. vendor/CrispASR (git submodule)
///   3. Fail with actionable error
///
/// In normal builds Cargo resolves the `crispasr` path dependency before this
/// function can run, so a completely missing submodule still has to be fixed
/// with `git submodule update --init --recursive`.
fn locate_source(manifest_dir: &Path) -> PathBuf {
    if let Ok(d) = env::var("CRISPASR_SRC_DIR") {
        let p = PathBuf::from(d);
        if p.join("CMakeLists.txt").is_file() {
            return p;
        }
        panic!(
            "CRISPASR_SRC_DIR={} does not contain CMakeLists.txt",
            p.display()
        );
    }

    let vendored = manifest_dir.join("vendor/CrispASR");
    if vendored.join("CMakeLists.txt").is_file() {
        return vendored;
    }

    panic!(
        "\n\
         parakit build.rs: cannot find CrispASR source.\n\
         \n\
         Pick one of:\n\
           1. Initialize the submodule:\n\
                git submodule update --init --recursive\n\
           2. Vendor a checkout manually:\n\
                git clone https://github.com/CrispStrobe/CrispASR vendor/CrispASR\n\
           3. Point at an existing checkout:\n\
                CRISPASR_SRC_DIR=/path/to/CrispASR cargo build\n\
           4. Use a system-installed library and skip the bundled build:\n\
                cargo build --no-default-features --features daemon\n\
              (or set CRISPASR_LIB_DIR=/path/to/libdir to override the search path)\n"
    );
}

fn detected_openblas(
    explicit_openblas: bool,
    include_conventional: bool,
) -> Option<OpenBlasInstall> {
    if target_is_windows() {
        return windows_openblas_from_env(explicit_openblas).map(OpenBlasInstall::Windows);
    }
    if !target_is_linux() {
        return None;
    }

    unix_openblas_from_configured_roots(explicit_openblas)
        .or_else(|| {
            include_conventional
                .then(unix_openblas_from_conventional_roots)
                .flatten()
        })
        .map(OpenBlasInstall::Unix)
}

fn windows_openblas_from_env(explicit_openblas: bool) -> Option<WindowsOpenBlas> {
    if !target_is_windows() {
        return None;
    }

    let import_kind = windows_openblas_import_kind();

    if let Some(root) = env_path("PARAKIT_OPENBLAS_ROOT") {
        if let Some(openblas) = find_windows_openblas(&root, import_kind) {
            return Some(openblas);
        }
        if explicit_openblas && !manual_blas_path_overrides_are_set() {
            panic!(
                "PARAKIT_OPENBLAS_ROOT is set but does not contain a usable Windows OpenBLAS install for the active target environment. \
                 Expected cblas.h under include/ or include/openblas/, a target-compatible import lib under lib/, and a runtime DLL under bin/."
            );
        }
        println!(
            "cargo:warning=parakit build: PARAKIT_OPENBLAS_ROOT={} is not a usable Windows OpenBLAS layout for this target",
            root.display()
        );
    }

    for root in configured_openblas_roots(true)
        .into_iter()
        .chain(conventional_openblas_roots(true))
    {
        if let Some(openblas) = find_windows_openblas(&root, import_kind) {
            return Some(openblas);
        }
    }

    if explicit_openblas && !manual_blas_path_overrides_are_set() {
        panic!(
            "PARAKIT_BLAS=openblas requested Windows OpenBLAS, but no usable target-compatible install was found. \
             Set PARAKIT_OPENBLAS_ROOT to a prefix containing include/, lib/, and bin/, \
             install OpenBLAS in a conventional Conda, CMake, or vcpkg prefix, \
             or provide BLAS_INCLUDE_DIRS and BLAS_LIBRARIES."
        );
    }

    None
}

fn unix_openblas_from_configured_roots(explicit_openblas: bool) -> Option<UnixOpenBlas> {
    if let Some(root) = env_path("PARAKIT_OPENBLAS_ROOT") {
        if let Some(openblas) = find_unix_openblas(&root) {
            return Some(openblas);
        }
        if explicit_openblas && !manual_blas_path_overrides_are_set() {
            panic!(
                "PARAKIT_OPENBLAS_ROOT is set but does not contain a usable Unix OpenBLAS install. \
                 Expected cblas.h under include/ and libopenblas under lib/ or lib64/."
            );
        }
        println!(
            "cargo:warning=parakit build: PARAKIT_OPENBLAS_ROOT={} is not a usable Unix OpenBLAS layout",
            root.display()
        );
    }

    configured_openblas_roots(false)
        .into_iter()
        .find_map(|root| find_unix_openblas(&root))
}

fn unix_openblas_from_conventional_roots() -> Option<UnixOpenBlas> {
    conventional_openblas_roots(false)
        .into_iter()
        .find_map(|root| find_unix_openblas(&root))
}

fn windows_openblas_import_kind() -> WindowsOpenBlasImportKind {
    if env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default() == "gnu" {
        WindowsOpenBlasImportKind::Gnu
    } else {
        WindowsOpenBlasImportKind::Msvc
    }
}

fn manual_blas_path_overrides_are_set() -> bool {
    env::var("BLAS_INCLUDE_DIRS").is_ok() && env::var("BLAS_LIBRARIES").is_ok()
}

/// Tell the linker to bake `dir` into the binary's rpath.
/// On Linux/BSD we ALSO emit `--disable-new-dtags` so the resulting
/// DT_RPATH (rather than DT_RUNPATH) applies transitively to the
/// binary's transitive shared-library dependencies. This is belt-and-
/// suspenders insurance — `CMAKE_INSTALL_RPATH=$ORIGIN` should already
/// make libcrispasr.so find its own ggml siblings, but `--disable-new-dtags`
/// keeps things working even on systems where the cmake rpath setting
/// gets stripped.
///
/// macOS: @rpath/install_name resolution is naturally transitive via the
/// dyld machinery; rpath alone is sufficient.
/// Windows: rpath is a Unix concept; DLL resolution happens differently
/// (PATH or alongside the .exe). We document this in the README.
fn emit_rpath(dir: &Path) {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        "linux" | "freebsd" | "netbsd" | "openbsd" | "dragonfly" => {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
            println!("cargo:rustc-link-arg=-Wl,--disable-new-dtags");
        }
        "macos" | "ios" => {
            println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.display());
        }
        _ => {
            // Windows / WASM / other — no-op.
        }
    }
}

/// Returns the right "$ORIGIN-style" token for `CMAKE_INSTALL_RPATH`.
/// Linux/BSD: `$ORIGIN` — the dynamic linker substitutes it with the
/// directory of the loading binary at runtime.
/// macOS: `@loader_path` — same idea, dyld syntax.
fn install_rpath_token() -> &'static str {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target_os.as_str() {
        "macos" | "ios" => "@loader_path",
        _ => "$ORIGIN",
    }
}

/// Ensure `lib_dir` contains the pinned CrispASR shared library name.
fn assert_crispasr_library_exists(lib_dir: &Path) {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    let lib_name = match target_os.as_str() {
        "macos" | "ios" => "libcrispasr.dylib",
        _ => "libcrispasr.so",
    };

    let lib_path = lib_dir.join(lib_name);
    if lib_path.exists() {
        return;
    }

    panic!(
        "CrispASR build did not produce {}. \
         cmake build may have failed silently — \
         check `cargo build -vv` output.",
        lib_path.display()
    );
}

/// Ensure Apple Metal builds installed the Metal backend sibling dylib.
fn assert_apple_metal_library(lib_dir: &Path, metal_enabled: bool) {
    if !target_is_apple() || !metal_enabled {
        return;
    }

    let lib_path = lib_dir.join("libggml-metal.dylib");
    if lib_path.exists() {
        return;
    }

    panic!(
        "Metal build did not produce {}. \
         GGML_METAL is enabled, but the runtime Metal backend dylib is missing.",
        lib_path.display()
    );
}
